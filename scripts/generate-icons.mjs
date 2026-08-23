// 生成应用图标源图与托盘图标。
//
// 仓库里不放二进制资产，图标由这个脚本从数学定义画出来，改配色只要改这里。
// 生成 1024×1024 的源图后，用 `pnpm tauri icon` 派生出各平台需要的全套尺寸。
//
//   node scripts/generate-icons.mjs
//
// 图形：圆角方块底 + 居中的白色 "C"（缺口朝右），对应 CHENOCR 的首字母。

import { deflateSync } from 'node:zlib';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');

/** 每个方向的超采样数，用来给圆角和字形边缘做抗锯齿。 */
const SUPERSAMPLE = 4;

const BACKGROUND_TOP = [10, 132, 255]; // Apple system blue
const BACKGROUND_BOTTOM = [0, 96, 223];
const GLYPH = [255, 255, 255];

function smoothCoverage(sampleHits) {
  return sampleHits / (SUPERSAMPLE * SUPERSAMPLE);
}

/** 圆角矩形的内部判定，坐标已归一化到 [0,1]。 */
function insideRoundedSquare(x, y, radius) {
  const cx = Math.min(Math.max(x, radius), 1 - radius);
  const cy = Math.min(Math.max(y, radius), 1 - radius);
  const dx = x - cx;
  const dy = y - cy;
  return dx * dx + dy * dy <= radius * radius;
}

/**
 * "C" 的内部判定：一个圆环，右侧挖掉一个楔形做出开口。
 * 归一化坐标，圆心在 (0.5, 0.5)。
 */
function insideC(x, y, outer, inner, gapDegrees) {
  const dx = x - 0.5;
  const dy = y - 0.5;
  const dist = Math.hypot(dx, dy);
  if (dist > outer || dist < inner) return false;

  const angle = Math.atan2(dy, dx); // -PI..PI，0 指向右
  const halfGap = (gapDegrees / 2) * (Math.PI / 180);
  return Math.abs(angle) > halfGap;
}

function renderAppIcon(size) {
  const pixels = Buffer.alloc(size * size * 4);
  const cornerRadius = 0.22;
  const outer = 0.33;
  const inner = 0.205;
  const gap = 78;

  for (let py = 0; py < size; py += 1) {
    for (let px = 0; px < size; px += 1) {
      let bgHits = 0;
      let glyphHits = 0;

      for (let sy = 0; sy < SUPERSAMPLE; sy += 1) {
        for (let sx = 0; sx < SUPERSAMPLE; sx += 1) {
          const x = (px + (sx + 0.5) / SUPERSAMPLE) / size;
          const y = (py + (sy + 0.5) / SUPERSAMPLE) / size;

          if (insideRoundedSquare(x, y, cornerRadius)) bgHits += 1;
          if (insideC(x, y, outer, inner, gap)) glyphHits += 1;
        }
      }

      const bgAlpha = smoothCoverage(bgHits);
      const glyphAlpha = smoothCoverage(glyphHits);

      const t = py / (size - 1);
      const base = BACKGROUND_TOP.map((c, i) =>
        Math.round(c + (BACKGROUND_BOTTOM[i] - c) * t),
      );

      // 字形叠在底色之上，两者都各自带抗锯齿覆盖率
      const rgb = base.map((c, i) => Math.round(c * (1 - glyphAlpha) + GLYPH[i] * glyphAlpha));

      const offset = (py * size + px) * 4;
      pixels[offset] = rgb[0];
      pixels[offset + 1] = rgb[1];
      pixels[offset + 2] = rgb[2];
      pixels[offset + 3] = Math.round(bgAlpha * 255);
    }
  }

  return { pixels, size };
}

/**
 * 托盘图标：只有字形，纯白 + 透明底。
 * macOS 的 template 模式要求单色，Windows 上白色描边在深浅任务栏都看得清。
 */
function renderTrayIcon(size) {
  const pixels = Buffer.alloc(size * size * 4);
  const outer = 0.42;
  const inner = 0.26;
  const gap = 78;

  for (let py = 0; py < size; py += 1) {
    for (let px = 0; px < size; px += 1) {
      let hits = 0;
      for (let sy = 0; sy < SUPERSAMPLE; sy += 1) {
        for (let sx = 0; sx < SUPERSAMPLE; sx += 1) {
          const x = (px + (sx + 0.5) / SUPERSAMPLE) / size;
          const y = (py + (sy + 0.5) / SUPERSAMPLE) / size;
          if (insideC(x, y, outer, inner, gap)) hits += 1;
        }
      }

      const offset = (py * size + px) * 4;
      pixels[offset] = 255;
      pixels[offset + 1] = 255;
      pixels[offset + 2] = 255;
      pixels[offset + 3] = Math.round(smoothCoverage(hits) * 255);
    }
  }

  return { pixels, size };
}

// ── 最小 PNG 编码器 ────────────────────────────────────────────────────────
const CRC_TABLE = (() => {
  const table = new Int32Array(256);
  for (let n = 0; n < 256; n += 1) {
    let c = n;
    for (let k = 0; k < 8; k += 1) {
      c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    table[n] = c;
  }
  return table;
})();

function crc32(buffer) {
  let c = 0xffffffff;
  for (const byte of buffer) {
    c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  }
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const typeAndData = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(typeAndData));
  return Buffer.concat([length, typeAndData, crc]);
}

function encodePng({ pixels, size }) {
  const header = Buffer.alloc(13);
  header.writeUInt32BE(size, 0);
  header.writeUInt32BE(size, 4);
  header[8] = 8; // 位深
  header[9] = 6; // 颜色类型 6 = RGBA
  header[10] = 0; // 压缩方法
  header[11] = 0; // 过滤方法
  header[12] = 0; // 非隔行

  // 每行前面加一个 0 表示不使用过滤器
  const stride = size * 4;
  const raw = Buffer.alloc((stride + 1) * size);
  for (let y = 0; y < size; y += 1) {
    raw[y * (stride + 1)] = 0;
    pixels.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride);
  }

  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', header),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

function write(relativePath, png) {
  const target = resolve(root, relativePath);
  mkdirSync(dirname(target), { recursive: true });
  writeFileSync(target, png);
  console.log(`${relativePath}  ${(png.length / 1024).toFixed(1)} KB`);
}

write('scripts/app-icon.png', encodePng(renderAppIcon(1024)));
write('src-tauri/icons/tray.png', encodePng(renderTrayIcon(64)));
