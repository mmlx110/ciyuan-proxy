// 包装 tauri CLI,把指定的 conf 文件作为 TAURI_CONFIG 环境变量喂给 cargo。
//
// 为什么需要这个脚本:
//   tauri CLI 的 `--config FILE` 在 2.11.x 上对 `plugins.updater.endpoints` 这类深层覆盖
//   似乎没有真正合并进最终配置(实测二进制里仍是 tauri.conf.json 里的原值),而
//   `tauri-build` 明确读 `TAURI_CONFIG` 环境变量做 json_patch::merge(见
//   tauri-build/src/lib.rs 的 try_build),所以我们直接把 conf 内容塞进 TAURI_CONFIG,
//   绕开 `--config` 这条疑似有问题的链路。
//
// 用法:
//   node scripts/tauri-env.mjs dev  src-tauri/tauri.dev.conf.json
//   node scripts/tauri-env.mjs build src-tauri/tauri.test.conf.json
// 不传 conf 文件时等价于 `tauri <cmd>`,不做任何覆盖。

import { spawn } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(__dirname, '..');

const [, , tauriCmd, confPath] = process.argv;

if (!tauriCmd) {
  console.error('用法: node scripts/tauri-env.mjs <dev|build|...> [conf 文件]');
  process.exit(1);
}

const env = { ...process.env };
if (confPath) {
  const absolute = resolve(ROOT, confPath);
  // 把内容塞进 TAURI_CONFIG,tauri-build 会做 json_patch::merge 覆盖到 tauri.conf.json 上
  env.TAURI_CONFIG = readFileSync(absolute, 'utf8');
  // 兼容部分老版本 CLI 的环境变量探测(没有副作用,留着无害)
  env.TAURI_CONFIG_FILE = absolute;
  console.log(`[tauri-env] TAURI_CONFIG <- ${absolute}`);
}

const tauriBin = process.platform === 'win32' ? 'tauri.cmd' : 'tauri';
const child = spawn(tauriBin, [tauriCmd, ...process.argv.slice(3)], {
  cwd: ROOT,
  env,
  stdio: 'inherit',
  shell: process.platform === 'win32',
});

child.on('exit', (code) => process.exit(code ?? 0));
child.on('error', (err) => {
  console.error(`[tauri-env] 启动 tauri 失败: ${err.message}`);
  process.exit(1);
});
