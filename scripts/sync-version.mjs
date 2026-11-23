// 以 src-tauri/tauri.conf.json 为权威源,同步版本号到 package.json 与 src-tauri/Cargo.toml。
// 用法:
//   node scripts/sync-version.mjs sync                    # 以权威源为准,同步到其他两个文件
//   node scripts/sync-version.mjs bump <patch|minor|major> # 在权威源中 bump 后再同步

import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(__dirname, '..');

const TAURI_CONF = resolve(ROOT, 'src-tauri', 'tauri.conf.json');
const PACKAGE_JSON = resolve(ROOT, 'package.json');
const CARGO_TOML = resolve(ROOT, 'src-tauri', 'Cargo.toml');

function bumpVersion(version, type) {
  const [major, minor, patch] = version.split('.').map(Number);
  switch (type) {
    case 'major':
      return `${major + 1}.0.0`;
    case 'minor':
      return `${major}.${minor + 1}.0`;
    case 'patch':
      return `${major}.${minor}.${patch + 1}`;
    default:
      throw new Error(`未知的 bump 类型: ${type}`);
  }
}

async function readTauriVersion() {
  const raw = await readFile(TAURI_CONF, 'utf8');
  // tauri.conf.json 是标准 JSON,直接 parse
  const conf = JSON.parse(raw);
  if (!conf.version) {
    throw new Error('tauri.conf.json 中未找到 version 字段');
  }
  return conf.version;
}

async function writeTauriVersion(version) {
  const raw = await readFile(TAURI_CONF, 'utf8');
  // 保持原 JSON 的缩进风格(2 空格),并稳定字段顺序
  const conf = JSON.parse(raw);
  conf.version = version;
  const next = JSON.stringify(conf, null, 2) + '\n';
  await writeFile(TAURI_CONF, next, 'utf8');
}

async function syncPackageJson(version) {
  const raw = await readFile(PACKAGE_JSON, 'utf8');
  const pkg = JSON.parse(raw);
  if (pkg.version === version) return false;
  pkg.version = version;
  await writeFile(PACKAGE_JSON, JSON.stringify(pkg, null, 2) + '\n', 'utf8');
  return true;
}

async function syncCargoToml(version) {
  const raw = await readFile(CARGO_TOML, 'utf8');
  // 仅替换 [package] 段下的 version = "..." 行
  const next = raw.replace(
    /^(\[package\][\s\S]*?\n)version\s*=\s*"[^"]+"/m,
    `$1version = "${version}"`,
  );
  if (next === raw) return false;
  await writeFile(CARGO_TOML, next, 'utf8');
  return true;
}

async function run() {
  const [, , sub, arg] = process.argv;

  if (sub !== 'sync' && sub !== 'bump') {
    console.error('用法: node scripts/sync-version.mjs sync | bump <patch|minor|major>');
    process.exit(1);
  }

  let version = await readTauriVersion();
  console.log(`权威版本号(tauri.conf.json): ${version}`);

  if (sub === 'bump') {
    if (!arg) {
      console.error('bump 需要指定 patch | minor | major');
      process.exit(1);
    }
    version = bumpVersion(version, arg);
    await writeTauriVersion(version);
    console.log(`bump 后新版本号: ${version}`);
  }

  const pkgChanged = await syncPackageJson(version);
  const cargoChanged = await syncCargoToml(version);

  console.log(`package.json: ${pkgChanged ? '已更新' : '已是最新'}`);
  console.log(`Cargo.toml:   ${cargoChanged ? '已更新' : '已是最新'}`);
  console.log(`全部完成,当前统一版本: ${version}`);
}

run().catch((err) => {
  console.error(err);
  process.exit(1);
});