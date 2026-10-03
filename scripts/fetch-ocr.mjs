// 下载识字引擎 RapidOCR-json（MIT）到 src-tauri/resources/ocr/，打包时随安装包分发。
//
//   corepack pnpm fetch-ocr            已存在且校验通过就跳过
//   corepack pnpm fetch-ocr --force    强制重新下载
//
// 发布包是 7z（约 70MB，含多语种模型），只取用到的 5 个文件（约 32MB）。
// 解压用系统自带的 tar（Windows 10 1803+ 自带 bsdtar，支持 7z）。
// GitHub 访问慢时可以用环境变量 CHENOCR_OCR_URL 指向镜像地址。

import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createWriteStream, existsSync, mkdirSync, readFileSync, rmSync, copyFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { fileURLToPath } from 'node:url';

const VERSION = 'v0.2.0';
const URL_DEFAULT = `https://github.com/hiroi-sora/RapidOCR-json/releases/download/${VERSION}/RapidOCR-json_${VERSION}.7z`;
const ARCHIVE_ROOT = `RapidOCR-json_${VERSION}`;

/** 要用的文件 → SHA-256。改版本时一并更新。 */
const FILES = {
  'RapidOCR-json.exe': '616bd43a4672ee9fcf4d1ca607d7d9ec413b45d489482064d97e00a7efa9d76e',
  'models/ch_PP-OCRv4_det_infer.onnx': 'd2a7720d45a54257208b1e13e36a8479894cb74155a5efe29462512d42f49da9',
  'models/ch_ppocr_mobile_v2.0_cls_infer.onnx': 'e47acedf663230f8863ff1ab0e64dd2d82b838fceb5957146dab185a89d6215c',
  'models/rec_ch_PP-OCRv4_infer.onnx': '48fc40f24f6d2a207a2b1091d3437eb3cc3eb6b676dc3ef9c37384005483683b',
  'models/dict_chinese.txt': '28b2362ad4ab2dc38769aa72feb535e3a9ddb3fd2a7585a05920e6393b1dc7f7',
};

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const target = join(root, 'src-tauri', 'resources', 'ocr');
const force = process.argv.includes('--force');

const sha256 = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');

function installed() {
  return Object.entries(FILES).every(([rel, hash]) => {
    const path = join(target, rel);
    return existsSync(path) && sha256(path) === hash;
  });
}

async function download(url, dest) {
  const res = await fetch(url, { redirect: 'follow' });
  if (!res.ok || !res.body) throw new Error(`下载失败：HTTP ${res.status} ${url}`);
  const total = Number(res.headers.get('content-length')) || 0;
  let done = 0;
  let shown = -1;
  const body = Readable.fromWeb(res.body);
  body.on('data', (chunk) => {
    done += chunk.length;
    const pct = total ? Math.floor((done / total) * 100) : -1;
    if (pct !== shown && pct % 10 === 0) {
      shown = pct;
      process.stdout.write(`  ${pct}%  ${(done / 1048576).toFixed(1)} MB\n`);
    }
  });
  await pipeline(body, createWriteStream(dest));
}

async function main() {
  if (process.platform !== 'win32') {
    console.log('RapidOCR-json 只有 Windows 版，当前系统跳过（macOS 用系统自带的识字）。');
    return;
  }
  if (!force && installed()) {
    console.log(`识字引擎已就绪：${target}`);
    return;
  }

  const url = process.env.CHENOCR_OCR_URL || URL_DEFAULT;
  const work = join(tmpdir(), `chenocr-ocr-${Date.now()}`);
  mkdirSync(work, { recursive: true });
  const archive = join(work, 'rapidocr.7z');
  try {
    console.log(`下载 ${url}`);
    await download(url, archive);

    console.log('解压…');
    // 显式用系统的 bsdtar：Git Bash 等环境里 PATH 上的 tar 是 GNU tar，解不了 7z
    const tar = join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'tar.exe');
    execFileSync(tar, ['-xf', archive, '-C', work], { stdio: 'inherit' });

    for (const [rel, hash] of Object.entries(FILES)) {
      const src = join(work, ARCHIVE_ROOT, rel);
      if (!existsSync(src)) throw new Error(`发布包里缺少 ${rel}`);
      const actual = sha256(src);
      if (actual !== hash) throw new Error(`${rel} 校验失败：期望 ${hash}，实际 ${actual}`);
      const dest = join(target, rel);
      mkdirSync(dirname(dest), { recursive: true });
      copyFileSync(src, dest);
    }
    console.log(`识字引擎已安装到 ${target}`);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

main().catch((err) => {
  console.error(err instanceof Error ? err.message : err);
  process.exit(1);
});
