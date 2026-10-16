// 把 film.html 一帧一帧截下来，合成配乐，输出 MP4。
//
//   node render.mjs                     # 中文版 → dato-ocr-promo.mp4
//   node render.mjs --lang en           # 英文版 → dato-ocr-promo-en.mp4
//   node render.mjs --still 3.5,14.6    # 只截几张静帧到 build/stills/，调画面用
//   node render.mjs --audio             # 只合成配乐到 build/soundtrack-zh.wav，调声音用
//   node render.mjs --fps 30 --workers 2
//
// 需要：Chromium（Playwright 自带的或 CHROMIUM_PATH 指定）、ffmpeg、python3 + numpy。
import { spawn } from 'node:child_process';
import { mkdirSync, writeFileSync, existsSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { cpus } from 'node:os';
import { chromium } from 'playwright-core';

const here = dirname(fileURLToPath(import.meta.url));
const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => {
    if (a.startsWith('--')) acc.push([a.slice(2), all[i + 1] && !all[i + 1].startsWith('--') ? all[i + 1] : true]);
    return acc;
  }, []),
);
const lang = args.lang === 'en' ? 'en' : 'zh';
const build = join(here, 'build');
mkdirSync(build, { recursive: true });

const pageUrl = `${pathToFileURL(join(here, 'film.html')).href}?render&lang=${lang}`;

async function openPage(browser) {
  const page = await browser.newPage({ viewport: { width: 1920, height: 1080 }, deviceScaleFactor: 1 });
  let failed = null;
  page.on('pageerror', (e) => {
    console.error('[page]', e.message);
    failed = e;
  });
  await page.goto(pageUrl);
  await page.waitForFunction(() => window.FILM && window.FILM.ready, null, { timeout: 60_000, polling: 200 }).catch((e) => {
    throw failed || e;
  });
  const cdp = await page.context().newCDPSession(page);
  const shot = async (t) => {
    await page.evaluate((tt) => window.FILM.seek(tt), t);
    const r = await cdp.send('Page.captureScreenshot', { format: 'png', optimizeForSpeed: true });
    return Buffer.from(r.data, 'base64');
  };
  return { page, shot };
}

function launch() {
  return chromium.launch({
    executablePath: process.env.CHROMIUM_PATH || undefined,
    args: ['--force-color-profile=srgb', '--font-render-hinting=none', '--disable-lcd-text', '--allow-file-access-from-files'],
  });
}

function run(cmd, argv, opts = {}) {
  return new Promise((resolve, reject) => {
    const p = spawn(cmd, argv, { stdio: opts.stdin ? ['pipe', 'inherit', 'inherit'] : 'inherit' });
    p.on('error', reject);
    p.on('exit', (code) => (code === 0 ? resolve() : reject(new Error(`${cmd} exited with ${code}`))));
    if (opts.stdin) opts.stdin(p.stdin);
  });
}

async function stills(list) {
  const dir = join(build, 'stills');
  mkdirSync(dir, { recursive: true });
  const browser = await launch();
  const { shot } = await openPage(browser);
  for (const t of list) {
    const file = join(dir, `${lang}-${t.toFixed(2)}.png`);
    writeFileSync(file, await shot(t));
    console.log(file);
  }
  await browser.close();
}

async function video() {
  const fps = Number(args.fps || 60);
  const browser0 = await launch();
  const probe = await openPage(browser0);
  const { duration, cues } = await probe.page.evaluate(() => ({ duration: window.FILM.duration, cues: window.FILM.cues }));
  await browser0.close();
  const from = Number(args.from || 0);
  const to = Number(args.to || duration);
  writeFileSync(join(build, `cues-${lang}.json`), JSON.stringify({ duration, cues }, null, 1));

  const total = Math.round((to - from) * fps);
  const workers = Math.max(1, Math.min(Number(args.workers || cpus().length), 8));
  const per = Math.ceil(total / workers);
  const started = Date.now();
  let done = 0;
  const segs = [];

  await Promise.all(
    Array.from({ length: workers }, async (_, w) => {
      const a = w * per;
      const b = Math.min(total, a + per);
      if (a >= b) return;
      const seg = join(build, `seg-${lang}-${w}.mkv`);
      segs[w] = seg;
      const browser = await launch();
      const { shot } = await openPage(browser);
      await run(
        'ffmpeg',
        ['-y', '-loglevel', 'error', '-f', 'image2pipe', '-framerate', String(fps), '-c:v', 'png', '-i', '-', '-c:v', 'libx264', '-preset', 'ultrafast', '-qp', '0', '-pix_fmt', 'yuv444p', seg],
        {
          stdin: async (stdin) => {
            for (let f = a; f < b; f++) {
              const png = await shot(from + f / fps);
              if (!stdin.write(png)) await new Promise((r) => stdin.once('drain', r));
              done++;
              if (done % 60 === 0) {
                const el = (Date.now() - started) / 1000;
                process.stdout.write(`\r${done}/${total} 帧  ${(done / el).toFixed(1)} fps  剩余约 ${Math.round(((total - done) * el) / done)}s   `);
              }
            }
            stdin.end();
          },
        },
      );
      await browser.close();
    }),
  );
  console.log(`\n画面完成，用时 ${Math.round((Date.now() - started) / 1000)}s`);

  const list = join(build, `segs-${lang}.txt`);
  writeFileSync(list, segs.filter(Boolean).map((s) => `file '${s}'`).join('\n'));

  const wav = join(build, `soundtrack-${lang}.wav`);
  if (!args['no-audio']) {
    await run('python3', [join(here, 'soundtrack.py'), join(build, `cues-${lang}.json`), wav]);
  }
  const out = args.out || join(here, lang === 'zh' ? 'dato-ocr-promo.mp4' : 'dato-ocr-promo-en.mp4');
  const audioIn = args['no-audio'] ? [] : ['-ss', String(from), '-i', wav];
  await run('ffmpeg', [
    '-y', '-loglevel', 'error',
    '-f', 'concat', '-safe', '0', '-i', list,
    ...audioIn,
    // CRF 21 + 码率上限：成片 40 MB 左右（GitHub 单文件超过 50 MB 会警告、100 MB 直接拒收）
    '-c:v', 'libx264', '-preset', 'slow', '-crf', String(args.crf || 21), '-maxrate', '9M', '-bufsize', '18M', '-pix_fmt', 'yuv420p',
    '-profile:v', 'high', '-tune', 'animation', '-g', String(fps * 2),
    '-x264-params', 'aq-mode=3',
    ...(args['no-audio'] ? [] : ['-c:a', 'aac', '-b:a', '256k', '-shortest']),
    '-movflags', '+faststart',
    out,
  ]);
  if (!args.keep) for (const s of segs.filter(Boolean)) if (existsSync(s)) rmSync(s);
  console.log(out);
}

/** 只导出 cue、合成配乐（不渲染画面），调声音用 */
async function audioOnly() {
  const browser = await launch();
  const probe = await openPage(browser);
  const { duration, cues } = await probe.page.evaluate(() => ({ duration: window.FILM.duration, cues: window.FILM.cues }));
  await browser.close();
  const json = join(build, `cues-${lang}.json`);
  writeFileSync(json, JSON.stringify({ duration, cues }, null, 1));
  await run('python3', [join(here, 'soundtrack.py'), json, join(build, `soundtrack-${lang}.wav`)]);
}

if (args.still) await stills(String(args.still).split(',').map(Number));
else if (args.audio) await audioOnly();
else await video();
