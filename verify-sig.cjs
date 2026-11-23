// 验证 sign-with-node.cjs 生成的 .sig 是否与 .pubkey 匹配
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');

const target = process.argv[2];
if (!target) { console.error('用法: node verify-sig.cjs <installer.exe>'); process.exit(1); }

const installer = target;
const sigPath = target + '.sig';
const pubPath = path.join(__dirname, 'src-tauri', 'ciyuan-proxy.key.pub');

const sigContent = fs.readFileSync(sigPath, 'utf8').trim();
const sigLines = sigContent.split(/\r?\n/);
const sigB64 = sigLines[1];
const trustedComment = sigLines[2];
const trustedSigB64 = sigLines[3];

const sigBuf = Buffer.from(sigB64, 'base64');
console.log('签名 box 长度:', sigBuf.length, '(期望 74)');
console.log('sig_alg:', sigBuf.subarray(0, 2).toString('ascii'));
console.log('keyId:', sigBuf.subarray(2, 10).toString('hex'));
console.log('Ed25519 签名长度:', sigBuf.subarray(10).length, '(期望 64)');

const raw = fs.readFileSync(pubPath, 'utf8').trim();
let pubB64;
if (raw.includes('\n')) {
  pubB64 = raw.split(/\r?\n/)[1];
} else {
  const decoded = Buffer.from(raw, 'base64').toString('utf8');
  pubB64 = decoded.split(/\r?\n/)[1];
}
const pubBuf = Buffer.from(pubB64, 'base64');
console.log('pubkey 长度:', pubBuf.length, '(期望 42)');
console.log('pubkey sig_alg:', pubBuf.subarray(0, 2).toString('ascii'));
console.log('pubkey keyId:', pubBuf.subarray(2, 10).toString('hex'));
console.log('keyId 匹配:', Buffer.compare(sigBuf.subarray(2, 10), pubBuf.subarray(2, 10)) === 0);

const pk = pubBuf.subarray(10, 42);
const ed25519Sig = sigBuf.subarray(10);
const fileBuf = fs.readFileSync(installer);

try {
  // 用 Node 原生 Ed25519 verify (需要 SPKI DER 格式)
  const spkiHeader = Buffer.from([0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00]);
  const spki = Buffer.concat([spkiHeader, pk]);
  const pubKey = crypto.createPublicKey({ key: spki, format: 'der', type: 'spki' });
  const ok = crypto.verify(null, fileBuf, pubKey, ed25519Sig);
  console.log('Ed25519 签名验证:', ok ? '通过 ✓' : '失败 ✗');

  const trustedSig = Buffer.from(trustedSigB64, 'base64');
  const trustedOk = crypto.verify(null, Buffer.from(trustedComment), pubKey, trustedSig);
  console.log('trusted comment 签名验证:', trustedOk ? '通过 ✓' : '失败 ✗');
} catch (e) {
  console.error('验签失败:', e.message);
}
