// 原始像素源：放大镜、取色、马赛克都直接读原始像素，不经过任何已叠加标注的画布。

export interface PixelSource {
  width: number;
  height: number;
  /** 越界返回 null */
  get(x: number, y: number): [number, number, number] | null;
  /** 把 [x, y, w, h] 区域的平均色算出来（马赛克用） */
  average(x: number, y: number, w: number, h: number): [number, number, number];
}

/** 解析我们自己编码的 24bpp 自下而上 BMP（`imaging::encode_bmp24`）。 */
export function bmpSource(buffer: ArrayBuffer): PixelSource | null {
  const view = new DataView(buffer);
  if (view.byteLength < 54 || view.getUint8(0) !== 0x42 || view.getUint8(1) !== 0x4d) return null;
  const dataOffset = view.getUint32(10, true);
  const width = view.getInt32(18, true);
  const rawHeight = view.getInt32(22, true);
  const bpp = view.getUint16(28, true);
  if (bpp !== 24 && bpp !== 32) return null;
  const height = Math.abs(rawHeight);
  const topDown = rawHeight < 0;
  const bytes = bpp / 8;
  const stride = (width * bytes + 3) & ~3;
  const data = new Uint8Array(buffer);
  const offsetOf = (x: number, y: number) => dataOffset + (topDown ? y : height - 1 - y) * stride + x * bytes;

  return makeSource(width, height, (x, y) => {
    const o = offsetOf(x, y);
    return [data[o + 2] ?? 0, data[o + 1] ?? 0, data[o] ?? 0];
  });
}

/** 任意 RGBA 像素数组（编辑器里的图片解码后）。 */
export function rgbaSource(width: number, height: number, data: Uint8ClampedArray): PixelSource {
  return makeSource(width, height, (x, y) => {
    const o = (y * width + x) * 4;
    return [data[o] ?? 0, data[o + 1] ?? 0, data[o + 2] ?? 0];
  });
}

/**
 * 基于位图的像素源，按 256×256 分块懒读取。编辑器里的长图（1080×32000）整张
 * getImageData 要 138MB，分块读只碰到马赛克实际经过的地方。
 */
export function bitmapSource(bitmap: ImageBitmap): PixelSource {
  const TILE = 256;
  const tiles = new Map<string, ImageData>();
  const canvas = new OffscreenCanvas(TILE, TILE);
  const ctx = canvas.getContext('2d', { willReadFrequently: true });
  const tileOf = (tx: number, ty: number): ImageData | null => {
    const key = `${tx},${ty}`;
    let tile = tiles.get(key);
    if (!tile && ctx) {
      ctx.clearRect(0, 0, TILE, TILE);
      ctx.drawImage(bitmap, tx * TILE, ty * TILE, TILE, TILE, 0, 0, TILE, TILE);
      tile = ctx.getImageData(0, 0, TILE, TILE);
      tiles.set(key, tile);
    }
    return tile ?? null;
  };
  return makeSource(bitmap.width, bitmap.height, (x, y) => {
    const tile = tileOf(Math.floor(x / TILE), Math.floor(y / TILE));
    if (!tile) return [0, 0, 0];
    const o = ((y % TILE) * TILE + (x % TILE)) * 4;
    return [tile.data[o] ?? 0, tile.data[o + 1] ?? 0, tile.data[o + 2] ?? 0];
  });
}

function makeSource(width: number, height: number, read: (x: number, y: number) => [number, number, number]): PixelSource {
  const clampX = (x: number) => Math.max(0, Math.min(width - 1, x));
  const clampY = (y: number) => Math.max(0, Math.min(height - 1, y));
  return {
    width,
    height,
    get(x, y) {
      if (x < 0 || y < 0 || x >= width || y >= height) return null;
      return read(Math.floor(x), Math.floor(y));
    },
    average(x, y, w, h) {
      const x0 = clampX(Math.floor(x));
      const y0 = clampY(Math.floor(y));
      const x1 = clampX(Math.ceil(x + w) - 1);
      const y1 = clampY(Math.ceil(y + h) - 1);
      // 大格子隔点采样，够用且快
      const step = Math.max(1, Math.floor(Math.min(x1 - x0 + 1, y1 - y0 + 1) / 8));
      let r = 0;
      let g = 0;
      let b = 0;
      let n = 0;
      for (let yy = y0; yy <= y1; yy += step) {
        for (let xx = x0; xx <= x1; xx += step) {
          const p = read(xx, yy);
          r += p[0];
          g += p[1];
          b += p[2];
          n += 1;
        }
      }
      return n ? [Math.round(r / n), Math.round(g / n), Math.round(b / n)] : [0, 0, 0];
    },
  };
}
