// 给打好的 DATO OCR.app 重新做一次本机签名，把它的"身份"固定成包标识。
//
//   node scripts/mac-sign.mjs [某个 .app 的路径]      # 不给路径就用 tauri build 刚打出来的那个
//
// 为什么要多这一步：没有开发者证书时只能做本机签名（ad-hoc），这种签名默认拿整个程序的哈希当身份，
// 每次重新编译都不一样。系统的「屏幕录制」「辅助功能」授权是记在这个身份上的 —— 于是每打一次包，
// 之前的授权全部作废，而系统设置里那一项还显示开着，表现就是"明明授权了，点截图还是没反应"。
// 这里把"指定要求"写死成只认包标识，以后重新打包授权就不会丢了。
//
// 只适合自己机器上用的包：这等于说"任何自称是这个包标识的本机签名程序都算它"。
// 要发给别人，用开发者证书签名（tauri.macos.conf.json 的 signingIdentity），那时不需要这个脚本。

import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

if (process.platform !== 'darwin') {
  console.error('这个脚本只在 macOS 上用');
  process.exit(1);
}

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const conf = JSON.parse(readFileSync(join(root, 'src-tauri', 'tauri.conf.json'), 'utf8'));

function builtApp() {
  // 编译产物目录可能被 .cargo/config.toml 挪到别处，问 cargo 最准
  const meta = JSON.parse(
    execFileSync('cargo', ['metadata', '--format-version', '1', '--no-deps', '--manifest-path', join(root, 'src-tauri', 'Cargo.toml')], {
      encoding: 'utf8',
      maxBuffer: 64 * 1024 * 1024,
    }),
  );
  return join(meta.target_directory, 'release', 'bundle', 'macos', `${conf.productName}.app`);
}

const app = process.argv[2] ? resolve(process.argv[2]) : builtApp();
if (!existsSync(app)) {
  console.error(`找不到 ${app}，先跑 tauri build`);
  process.exit(1);
}

const id = conf.identifier;
execFileSync(
  'codesign',
  ['--force', '--deep', '--sign', '-', '--options', 'runtime', '--identifier', id, `-r=designated => identifier "${id}"`, app],
  { stdio: 'inherit' },
);
execFileSync('codesign', ['--verify', '--deep', '--strict', app], { stdio: 'inherit' });
console.log(`已签名：${app}\n身份固定为包标识 ${id}，重新打包不会再丢系统授权`);
