// 独立 minisign 签名脚本,绕过 npx tauri signer sign 对 "encrypted" 空密码密钥的 bug
// 读取 rsign2 格式 .key 文件,提取 Ed25519 私钥,用 Node crypto 做签名,输出 minisign 格式 .sig
//
// key 文件格式(共 158 字节 base64 解码后):
//   sig_alg(2)  "Ed" 标识 Ed25519
//   kdf_alg(2)  "Sc" 标识 scrypt 加密, [0,0] 标识无加密
//   chk_alg(2)  "B2" 标识 BLAKE2b-256
//   kdf_salt(32)
//   opslimit(8)   u64 LE
//   memlimit(8)   u64 LE
//   keynum(8)
//   sk(64)        Ed25519 私钥 (前 32 字节是 seed,后 32 字节是 pk)
//   chk(32)       BLAKE2b(sig_alg || keynum || sk)
//
// 用法: node sign-with-node.cjs <installer-path>

const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const blakejs = require('blakejs');

function blake2b256(data) {
  return Buffer.from(blakejs.blake2b(data, null, 32));
}

function readMinisignKey(keyPath, password = '') {
  const raw = fs.readFileSync(keyPath, 'utf8').trim();
  let keynumB64;

  if (raw.includes('\n')) {
    const lines = raw.split(/\r?\n/);
    keynumB64 = lines[1];
  } else {
    // 整文件是 base64(Tauri CLI 的包装格式)
    const decoded = Buffer.from(raw, 'base64').toString('utf8');
    const lines = decoded.split(/\r?\n/);
    keynumB64 = lines[1];
  }

  const keynum = Buffer.from(keynumB64, 'base64');
  if (keynum.length !== 158) {
    throw new Error(`keynum 长度异常: ${keynum.length} (期望 158)`);
  }

  const sigAlg = keynum.subarray(0, 2);
  const kdfAlg = keynum.subarray(2, 4);
  const chkAlg = keynum.subarray(4, 6);
  const kdfSalt = keynum.subarray(6, 38);
  const opsLimit = keynum.readBigUInt64LE(38);
  const memLimit = keynum.readBigUInt64LE(46);
  let keynumBytes = Buffer.from(keynum.subarray(54, 62));
  let sk = Buffer.from(keynum.subarray(62, 126));
  let chk = Buffer.from(keynum.subarray(126, 158));

  if (sigAlg.toString() !== 'Ed') {
    throw new Error(`不支持的 sig_alg: ${sigAlg.toString()}`);
  }
  if (chkAlg.toString() !== 'B2') {
    throw new Error(`不支持的 chk_alg: ${chkAlg.toString()}`);
  }

  if (kdfAlg.toString() === 'Sc') {
    // 加密: scrypt 派生长流,XOR 解密
    // minisign 用的是 libsodium 风格,选最大 N 满足 ops=2*N*r ≤ opslimit && mem=128*N*r ≤ memlimit
    // 默认 r=8, p=1, log2_n_max=20
    let nLog2 = 20;
    const r = 8;
    const p = 1;
    const opsLimitN = Number(opsLimit);
    const memLimitN = Number(memLimit);
    while (nLog2 > 1) {
      const n = 1 << nLog2;
      const ops = 2 * n * r;
      const mem = 128 * n * r;
      if (ops <= opsLimitN && mem <= memLimitN) break;
      nLog2 -= 1;
    }
    const N = 1 << nLog2;
    const maxmem = memLimitN + 128 * 1024;

    const stream = crypto.scryptSync(
      Buffer.from(password, 'utf8'),
      kdfSalt,
      8 + 64 + 32,  // keynum + sk + chk
      { N, r, p, maxmem }
    );

    // XOR 解密
    for (let i = 0; i < 8; i++) keynumBytes[i] ^= stream[i];
    for (let i = 0; i < 64; i++) sk[i] ^= stream[8 + i];
    for (let i = 0; i < 32; i++) chk[i] ^= stream[8 + 64 + i];
  } else if (kdfAlg[0] === 0 && kdfAlg[1] === 0) {
    // 无加密,直接用
  } else {
    throw new Error(`不支持的 kdf_alg: ${kdfAlg.toString('hex')}`);
  }

  // 校验: BLAKE2b-256(sig_alg || keynum || sk) == chk
  const h = blake2b256(Buffer.concat([sigAlg, keynumBytes, sk]));
  if (!h.equals(chk)) {
    throw new Error('BLAKE2b 校验和验证失败,密码可能错误');
  }

  return { sk, pk: sk.subarray(32, 64), keynum: keynumBytes };
}

function readPubKey(pubPath) {
  const raw = fs.readFileSync(pubPath, 'utf8').trim();
  let pubB64;
  if (raw.includes('\n')) {
    const lines = raw.split(/\r?\n/);
    pubB64 = lines[1];
  } else {
    const decoded = Buffer.from(raw, 'base64').toString('utf8');
    const lines = decoded.split(/\r?\n/);
    pubB64 = lines[1];
  }
  const pub = Buffer.from(pubB64, 'base64');
  if (pub.length !== 42) {
    throw new Error(`pubkey 长度异常: ${pub.length} (期望 42)`);
  }
  // 格式: "Ed"(2) + keynum(8) + pk(32) = 42
  if (pub[0] !== 0x45 || pub[1] !== 0x64) {
    throw new Error(`pubkey sig_alg 异常: ${pub.subarray(0, 2).toString('hex')} (期望 "Ed")`);
  }
  return { keyId: pub.subarray(2, 10), pk: pub.subarray(10, 42) };
}

function buildEd25519Pkcs8(seed) {
  // PKCS#8 DER for Ed25519 private key (RFC 8410)
  return Buffer.concat([
    Buffer.from([
      0x30, 0x2e, 0x02, 0x01, 0x00,
      0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70,
      0x04, 0x22, 0x04, 0x20
    ]),
    seed
  ]);
}

function signMinisign(installerPath, keyPath, pubPath, password) {
  const { sk, pk, keynum } = readMinisignKey(keyPath, password);
  const pubInfo = readPubKey(pubPath);

  if (!pk.equals(pubInfo.pk)) {
    throw new Error('.key 提取的 pk 与 .pub 不匹配!');
  }

  const fileBuf = fs.readFileSync(installerPath);
  const fileHash = crypto.createHash('blake2b512').update(fileBuf).digest();

  // Ed25519 签名
  const edSk = buildEd25519Pkcs8(sk.subarray(0, 32));
  const keyObj = crypto.createPrivateKey({ key: edSk, format: 'der', type: 'pkcs8' });
  const sig = crypto.sign(null, fileBuf, keyObj);

  // minisign signature box: "Ed"(2) + keynum(8) + sig(64) = 74 字节
  const minisignSig = Buffer.concat([
    Buffer.from([0x45, 0x64]),  // "Ed"
    pubInfo.keyId,
    sig
  ]);

  const trustedComment = `timestamp:${Math.floor(Date.now() / 1000)}\tfile:${path.basename(installerPath)}\thashed`;
  const trustedCommentSig = crypto.sign(null, Buffer.from(trustedComment), keyObj);

  const sigB64 = minisignSig.toString('base64');
  const sigPath = installerPath + '.sig';
  const sigContent =
    'untrusted comment: signature from minisign\n' +
    sigB64 + '\n' +
    trustedComment + '\n' +
    trustedCommentSig.toString('base64') + '\n';
  fs.writeFileSync(sigPath, sigContent, 'utf8');

  console.log('签名成功:', sigPath);
  console.log('  签名长度:', minisignSig.length, '字节');
  console.log('  keyId:', pubInfo.keyId.toString('hex'));
  console.log('  trusted comment:', trustedComment);
}

const installerPath = process.argv[2];
if (!installerPath) {
  console.error('用法: node sign-with-node.cjs <installer.exe> [私钥密码]');
  console.error('密码也可通过环境变量 TAURI_SIGNING_PRIVATE_KEY_PASSWORD 传入');
  process.exit(1);
}
const keyPath = path.join(__dirname, 'src-tauri', 'ciyuan-proxy.key');
const pubPath = path.join(__dirname, 'src-tauri', 'ciyuan-proxy.key.pub');

if (!fs.existsSync(keyPath) || !fs.existsSync(pubPath)) {
  console.error('找不到 .key 或 .key.pub:', keyPath, pubPath);
  process.exit(1);
}

const password = process.argv[3] ?? process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ?? '';
if (!password) {
  console.warn('[warn] 未提供私钥密码,按空密码尝试解密(仅当密钥未加密时有效)');
}

try {
  signMinisign(installerPath, keyPath, pubPath, password);
} catch (e) {
  console.error('签名失败:', e.message);
  process.exit(1);
}
