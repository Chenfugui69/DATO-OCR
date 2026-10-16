// 发版：打安装包 → 用更新私钥签名 → 生成国内 / 国外两份更新清单。
// 在哪台电脑上跑就发哪个平台：Windows 上打安装包（.exe），Mac 上打更新包（.app.tar.gz）和给人手动下载的 .dmg。
// 两个平台在两台电脑上各跑一次；清单按版本号合并——同一个版本已经有另一个平台的条目就保留，
// 版本号变了就只留这次的（旧版本的条目配新版本号，客户端验签也过不了，留着没用）。
//
//   node scripts/release.mjs              # 打包 + 签名 + 生成清单
//   node scripts/release.mjs --skip-build # 安装包已经打好了，只签名和生成清单
//   node scripts/release.mjs --github-only # Gitee 还没准备好：只生成国外清单。国内渠道读不到清单时会自动改走 GitHub，
//                                           # 比写一份指向不存在的 Gitee 下载地址的清单安全
//
// 之前要做的：
//   1. 三处版本号改成一样的新版本：package.json、src-tauri/Cargo.toml、src-tauri/tauri.conf.json
//   2. 写更新简介 update/notes/<版本>.json（added / fixed / improved，给不懂技术的用户看，一条一句话）
//   3. 私钥在 ~/.tauri/dato-cor-updater.key（或者用环境变量 TAURI_SIGNING_PRIVATE_KEY_PATH 指过去）
//
// 生成的东西：
//   release/<版本>/DATO-OCR_<版本>_x64-setup.exe(.sig)          Windows：上传到 GitHub 和 Gitee 的发行版（标签 v<版本>）
//   release/<版本>/DATO-OCR_<版本>_macos-arm64.app.tar.gz(.sig) Mac 自动更新用的包：同样上传
//   release/<版本>/DATO-OCR_<版本>_macos-arm64.dmg              Mac 手动下载安装用：同样上传（不进清单）
//   update/latest.json      国外渠道的清单（下载地址指向 GitHub）
//   update/latest-cn.json   国内渠道的清单（下载地址指向 Gitee）
//
// 顺序很重要：先把安装包传到两边的发行版，再提交、推送 update/ 下的清单（同步到 Gitee）。
// 清单一推上去，所有开着自动检查的用户就会看到更新。

import { execFileSync } from 'node:child_process';
import { createHash, createPublicKey, verify as edVerify } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { delimiter, join, resolve } from 'node:path';

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

const MAC = process.platform === 'darwin';
// rustup 装的 cargo 不一定在 PATH 里（Mac 上常见）：给子进程补上
const cargoBin = join(homedir(), '.cargo', 'bin');
if (existsSync(cargoBin) && !(process.env.PATH ?? '').split(delimiter).includes(cargoBin)) {
  process.env.PATH = `${cargoBin}${delimiter}${process.env.PATH ?? ''}`;
}

const keyPath = process.env.TAURI_SIGNING_PRIVATE_KEY_PATH || join(homedir(), '.tauri', 'dato-cor-updater.key');
if (!existsSync(keyPath)) fail(`找不到更新签名私钥 ${keyPath}`);

// 直接用 node 跑 Tauri CLI：不经过 shell，参数（比如空密码 ""）原样传过去
const cli = join(ROOT, 'node_modules/@tauri-apps/cli/tauri.js');
if (!existsSync(cli)) fail('找不到 Tauri CLI，先 corepack pnpm install');
const run = (args) => execFileSync(process.execPath, [cli, ...args], { cwd: ROOT, stdio: 'inherit' });

const skipBuild = process.argv.includes('--skip-build');
const outDir = join(ROOT, 'release', version);
mkdirSync(outDir, { recursive: true });

// 文件名不带空格（GitHub 会把空格换成点，Gitee 的处理不确定），签名里的文件名也就带着版本号
let name; // 进清单、要签名的那个文件
let platformKey; // 清单里的平台名，和 update.rs 的 platform_key 一致
const extraFiles = []; // 也要上传、但不进清单的
if (MAC) {
  const arch = process.arch === 'arm64' ? 'arm64' : 'x64';
  platformKey = arch === 'arm64' ? 'darwin-aarch64' : 'darwin-x86_64';
  // 编译产物目录可能被 .cargo/config.toml 挪到别处，问 cargo 最准
  const meta = JSON.parse(
    execFileSync('cargo', ['metadata', '--format-version', '1', '--no-deps', '--manifest-path', join(ROOT, 'src-tauri/Cargo.toml')], {
      encoding: 'utf8',
      maxBuffer: 64 * 1024 * 1024,
    }),
  );
  const productName = JSON.parse(readFileSync(join(ROOT, 'src-tauri/tauri.conf.json'), 'utf8')).productName;
  const bundleDir = join(meta.target_directory, 'release', 'bundle', 'macos');
  const app = join(bundleDir, `${productName}.app`);
  if (!skipBuild) {
    console.log(`\n▶ 打包 ${version}（macOS）…`);
    run(['build', '--bundles', 'app']);
    // 固定签名身份：换了 .app 系统授权也还在（自动更新靠这个）
    execFileSync(process.execPath, [join(ROOT, 'scripts/mac-sign.mjs'), app], { stdio: 'inherit' });
  }
  if (!existsSync(app)) fail(`找不到 ${app}`);
  const appVersion = execFileSync('/usr/bin/plutil', ['-extract', 'CFBundleShortVersionString', 'raw', '-o', '-', join(app, 'Contents/Info.plist')], {
    encoding: 'utf8',
  }).trim();
  if (appVersion !== version) fail(`${app} 是 ${appVersion}，不是 ${version}，去掉 --skip-build 重新打包`);

  // 自动更新用的包：整个 .app 打成 tar.gz。COPYFILE_DISABLE 不让 tar 带上 ._ 文件，否则解开后签名验不过
  name = `DATO-OCR_${version}_macos-${arch}.app.tar.gz`;
  console.log('\n▶ 打更新包和 DMG…');
  execFileSync('/usr/bin/tar', ['-czf', join(outDir, name), '-C', bundleDir, `${productName}.app`], {
    env: { ...process.env, COPYFILE_DISABLE: '1' },
    stdio: 'inherit',
  });

  // 手动下载用的 DMG：.app 加一个指向「应用程序」的快捷方式。
  // 中转目录放在编译产物旁边（APFS），放在 exFAT 的项目盘上会被塞进 ._ 文件
  const dmg = `DATO-OCR_${version}_macos-${arch}.dmg`;
  const staging = join(meta.target_directory, 'dmg-staging');
  rmSync(staging, { recursive: true, force: true });
  mkdirSync(staging, { recursive: true });
  execFileSync('/usr/bin/ditto', [app, join(staging, `${productName}.app`)]);
  symlinkSync('/Applications', join(staging, 'Applications'));
  const dmgTmp = join(meta.target_directory, dmg);
  execFileSync('/usr/bin/hdiutil', ['create', '-volname', productName, '-srcfolder', staging, '-fs', 'HFS+', '-format', 'UDZO', '-imagekey', 'zlib-level=9', '-ov', dmgTmp], {
    stdio: 'inherit',
  });
  rmSync(staging, { recursive: true, force: true });
  copyFileSync(dmgTmp, join(outDir, dmg));
  extraFiles.push(dmg);
} else {
  platformKey = 'windows-x86_64';
  if (!skipBuild) {
    console.log(`\n▶ 打包 ${version}…`);
    run(['build']);
  }
  const built = join(ROOT, `src-tauri/target/release/bundle/nsis/DATO OCR_${version}_x64-setup.exe`);
  if (!existsSync(built)) fail(`找不到安装包 ${built}`);
  name = `DATO-OCR_${version}_x64-setup.exe`;
  copyFileSync(built, join(outDir, name));
}
const installer = join(outDir, name);

console.log('\n▶ 签名…');
run(['signer', 'sign', '-f', keyPath, '-p', process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ?? '', installer]);
const signature = readFileSync(`${installer}.sig`, 'utf8').trim();
const size = statSync(installer).size;

// 自检：用程序里内置的公钥（update.rs 的 PUBKEY）验一遍，确认签名的私钥和发出去的程序是一对。
// minisign 格式：签名行 = 算法(2) ‖ 钥匙 ID(8) ‖ 签名(64)，"ED" 表示对 BLAKE2b-512 摘要签名；
// 全局签名 = 对（签名 ‖ 可信注释）再签一次。
function selfCheck() {
  const pubkeyB64 = /const PUBKEY: &str = "([^"]+)"/.exec(readFileSync(join(ROOT, 'src-tauri/src/update.rs'), 'utf8'))?.[1];
  if (!pubkeyB64) fail('update.rs 里找不到 PUBKEY');
  const pkLine = Buffer.from(pubkeyB64, 'base64').toString('utf8').trim().split('\n')[1];
  const pk = Buffer.from(pkLine, 'base64');
  const sigLines = Buffer.from(signature, 'base64').toString('utf8').split('\n');
  const sig = Buffer.from(sigLines[1], 'base64');
  const trusted = sigLines[2].replace(/^trusted comment: /, '');
  const globalSig = Buffer.from(sigLines[3], 'base64');
  if (!pk.subarray(2, 10).equals(sig.subarray(2, 10))) fail('签名用的私钥和程序里的公钥不是一对（钥匙 ID 不同）');
  // Ed25519 公钥裸字节包成 SPKI DER，node:crypto 才认
  const key = createPublicKey({
    key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), pk.subarray(10, 42)]),
    format: 'der',
    type: 'spki',
  });
  const data = readFileSync(installer);
  const message = sig.subarray(0, 2).toString() === 'ED' ? createHash('blake2b512').update(data).digest() : data;
  if (!edVerify(null, message, key, sig.subarray(10, 74))) fail('安装包签名验不过');
  if (!edVerify(null, Buffer.concat([sig.subarray(10, 74), Buffer.from(trusted)]), key, globalSig)) fail('可信注释验不过');
  if (!trusted.includes(`file:${name}`)) fail(`签名里的文件名不对：${trusted}`);
  console.log(`✓ 签名自检通过（${trusted}）`);
}
selfCheck();

// 纯文本简介（旧客户端 / 没有 changes 字段时用）和 GitHub 发行版页面的说明
const SECTIONS = [
  ['added', '新增'],
  ['improved', '改进'],
  ['fixed', '修复'],
];
const plainNotes = SECTIONS.filter(([k]) => notes[k]?.length)
  .map(([k, label]) => `${label}：${notes[k].join('；')}`)
  .join('\n');
const releaseMd = [
  ...SECTIONS.filter(([k]) => notes[k]?.length).flatMap(([k, label]) => [`### ${label}`, '', ...notes[k].map((x) => `- ${x}`), '']),
  '### 下载',
  '',
  `- **Windows**：下载 \`DATO-OCR_${version}_x64-setup.exe\` 双击安装`,
  `- **macOS（Apple 芯片）**：下载 \`DATO-OCR_${version}_macos-arm64.dmg\`，把 DATO OCR 拖进「应用程序」`,
  '',
  '已经装了旧版的，打开「设置 → 更新」检查更新就能直接升级。',
  '',
].join('\n');

// 同一个版本已经有别的平台（另一台电脑上发的）就合进去；版本号不同就重新开始
function readManifest(file) {
  try {
    return JSON.parse(readFileSync(file, 'utf8'));
  } catch {
    return null;
  }
}
const manifest = (file, base) => {
  const prev = readManifest(file);
  const kept = prev?.version === version ? (prev.platforms ?? {}) : {};
  return {
    version,
    pub_date: new Date().toISOString(),
    notes: plainNotes,
    changes: { added: notes.added ?? [], fixed: notes.fixed ?? [], improved: notes.improved ?? [] },
    ...(notes.en ? { changesEn: { added: notes.en.added ?? [], fixed: notes.en.fixed ?? [], improved: notes.en.improved ?? [] } } : {}),
    platforms: {
      ...kept,
      [platformKey]: { signature, url: `${base}/v${version}/${name}`, size },
    },
  };
};
const writeManifest = (file, base) => {
  const m = manifest(file, base);
  writeFileSync(file, `${JSON.stringify(m, null, 2)}\n`);
  return Object.keys(m.platforms);
};

writeFileSync(join(outDir, 'RELEASE_NOTES.md'), releaseMd);
mkdirSync(join(ROOT, 'update'), { recursive: true });
const platforms = writeManifest(join(ROOT, 'update/latest.json'), GITHUB);
const githubOnly = process.argv.includes('--github-only');
if (!githubOnly) {
  writeManifest(join(ROOT, 'update/latest-cn.json'), GITEE);
}
const uploads = [name, ...extraFiles].map((f) => `release/${version}/${f}`).join('、');
const others = ['windows-x86_64', 'darwin-aarch64'].filter((k) => !platforms.includes(k));
const otherNote = others.length
  ? `\n  清单里还没有 ${others.join('、')}：到那台电脑上也跑一次本脚本（先拉下这次的清单），清单会合并`
  : '';

if (githubOnly) {
  console.log(`
✓ ${version} 准备好了（${platformKey}，${(size / 1024 / 1024).toFixed(1)} MB，只生成了国外清单，里面有 ${platforms.join('、')}）

接下来：
  1. GitHub 发行版（标签 v${version}，没有就新建）上传 ${uploads}；
     说明可以直接粘贴 release/${version}/RELEASE_NOTES.md
  2. 提交并推送 update/latest.json —— 推上去之后，开着自动检查的用户就会收到提示
  Gitee 准备好以后：Gitee 发行版上传同样的文件，再跑一次 node scripts/release.mjs --skip-build 生成两份清单${otherNote}
`);
  process.exit(0);
}

console.log(`
✓ ${version} 准备好了（${platformKey}，${(size / 1024 / 1024).toFixed(1)} MB，清单里有 ${platforms.join('、')}）

接下来：
  1. GitHub 发行版（标签 v${version}，没有就新建）上传 ${uploads}
  2. Gitee 发行版（标签 v${version}）上传同样的文件
  3. 提交并推送 update/latest.json、update/latest-cn.json（Gitee 仓库同步一下）
     —— 推上去之后，开着自动检查的用户就会收到提示${otherNote}
`);
