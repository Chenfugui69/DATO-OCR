// 发版：打安装包 → 用更新私钥签名 → 生成国内 / 国外两份更新清单。
//
//   node scripts/release.mjs              # 打包 + 签名 + 生成清单
//   node scripts/release.mjs --skip-build # 安装包已经打好了，只签名和生成清单
//
// 之前要做的：
//   1. 三处版本号改成一样的新版本：package.json、src-tauri/Cargo.toml、src-tauri/tauri.conf.json
//   2. 写更新简介 update/notes/<版本>.json（added / fixed / improved，给不懂技术的用户看，一条一句话）
//   3. 私钥在 ~/.tauri/dato-cor-updater.key（或者用环境变量 TAURI_SIGNING_PRIVATE_KEY_PATH 指过去）
//
// 生成的东西：
//   release/<版本>/DATO-COR_<版本>_x64-setup.exe(.sig)   上传到 GitHub 和 Gitee 的发行版（标签 v<版本>）
//   update/latest.json      国外渠道的清单（下载地址指向 GitHub）
//   update/latest-cn.json   国内渠道的清单（下载地址指向 Gitee）
//
// 顺序很重要：先把安装包传到两边的发行版，再提交、推送 update/ 下的清单（同步到 Gitee）。
// 清单一推上去，所有开着自动检查的用户就会看到更新。

import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';

const ROOT = resolve(import.meta.dirname, '..');
const GITHUB = 'https://github.com/Chenfugui69/DATO-OCR/releases/download';
const GITEE = 'https://gitee.com/Chenfugui69/DATO-OCR/releases/download';

function fail(msg) {
  console.error(`\n✗ ${msg}\n`);
  process.exit(1);
}

const pkg = JSON.parse(readFileSync(join(ROOT, 'package.json'), 'utf8')).version;
const conf = JSON.parse(readFileSync(join(ROOT, 'src-tauri/tauri.conf.json'), 'utf8')).version;
const cargo = /^version\s*=\s*"([^"]+)"/m.exec(readFileSync(join(ROOT, 'src-tauri/Cargo.toml'), 'utf8'))?.[1];
if (!(pkg === conf && conf === cargo)) {
  fail(`三处版本号不一致：package.json ${pkg}，tauri.conf.json ${conf}，Cargo.toml ${cargo}`);
}
const version = conf;

const notesPath = join(ROOT, `update/notes/${version}.json`);
if (!existsSync(notesPath)) fail(`缺少更新简介 ${notesPath}`);
const notes = JSON.parse(readFileSync(notesPath, 'utf8'));
for (const k of ['added', 'fixed', 'improved']) {
  if (notes[k] && !Array.isArray(notes[k])) fail(`${notesPath} 里的 ${k} 应该是一个列表`);
}

const keyPath = process.env.TAURI_SIGNING_PRIVATE_KEY_PATH || join(homedir(), '.tauri', 'dato-cor-updater.key');
if (!existsSync(keyPath)) fail(`找不到更新签名私钥 ${keyPath}`);

// 直接用 node 跑 Tauri CLI：不经过 shell，参数（比如空密码 ""）原样传过去
const cli = join(ROOT, 'node_modules/@tauri-apps/cli/tauri.js');
if (!existsSync(cli)) fail('找不到 Tauri CLI，先 corepack pnpm install');
const run = (args) => execFileSync(process.execPath, [cli, ...args], { cwd: ROOT, stdio: 'inherit' });

if (!process.argv.includes('--skip-build')) {
  console.log(`\n▶ 打包 ${version}…`);
  run(['build']);
}

const built = join(ROOT, `src-tauri/target/release/bundle/nsis/DATO COR_${version}_x64-setup.exe`);
if (!existsSync(built)) fail(`找不到安装包 ${built}`);

// 文件名不带空格（GitHub 会把空格换成点，Gitee 的处理不确定），签名里的文件名也就带着版本号
const name = `DATO-COR_${version}_x64-setup.exe`;
const outDir = join(ROOT, 'release', version);
mkdirSync(outDir, { recursive: true });
const installer = join(outDir, name);
copyFileSync(built, installer);

console.log('\n▶ 签名…');
run(['signer', 'sign', '-f', keyPath, '-p', process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ?? '', installer]);
const signature = readFileSync(`${installer}.sig`, 'utf8').trim();
const size = statSync(installer).size;

const manifest = (base) => ({
  version,
  pub_date: new Date().toISOString(),
  notes: [...(notes.added ?? []), ...(notes.fixed ?? []), ...(notes.improved ?? [])].join('；'),
  changes: { added: notes.added ?? [], fixed: notes.fixed ?? [], improved: notes.improved ?? [] },
  ...(notes.en ? { changesEn: { added: notes.en.added ?? [], fixed: notes.en.fixed ?? [], improved: notes.en.improved ?? [] } } : {}),
  platforms: {
    'windows-x86_64': { signature, url: `${base}/v${version}/${name}`, size },
  },
});

mkdirSync(join(ROOT, 'update'), { recursive: true });
writeFileSync(join(ROOT, 'update/latest.json'), `${JSON.stringify(manifest(GITHUB), null, 2)}\n`);
writeFileSync(join(ROOT, 'update/latest-cn.json'), `${JSON.stringify(manifest(GITEE), null, 2)}\n`);

console.log(`
✓ ${version} 准备好了（${(size / 1024 / 1024).toFixed(1)} MB）

接下来：
  1. GitHub 新建发行版，标签 v${version}，上传 release/${version}/${name}
  2. Gitee 新建发行版，标签 v${version}，上传同一个文件
  3. 提交并推送 update/latest.json、update/latest-cn.json（Gitee 仓库同步一下）
     —— 推上去之后，开着自动检查的用户就会收到提示
`);
