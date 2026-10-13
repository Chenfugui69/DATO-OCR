// 划词按钮"液态玻璃"的折射：把按钮底下那块画面按一颗凸起的玻璃珠的样子重新取样，画到画布上。
//
// 系统的液态玻璃（和 liquidglassdesign.com 上那些例子）最像玻璃的地方在边缘：中间的画面基本原样（略微放大），
// 越靠近边缘光线折得越厉害，画面被压着沿圆边弯过去，红绿蓝三色折的程度还不一样，边上带一圈细细的彩边。
// CSS 的模糊 + 放大做不出这个（那只是一块均匀的毛玻璃），所以逐个像素算：珠子才几十个像素宽，一次几毫秒。
//
// 用 SVG 的位移滤镜（feDisplacementMap）也能做，但 WebKit 和 WebView2 对它的支持不一样，画布两边都一样。

/** 中间这么大一圈不折射（占半径的比例），外面这一圈是"斜边" */
const FLAT = 0.4;
/** 最边上往圆心拉回多远（占半径的比例）：越大边缘弯得越厉害 */
const BEND = 0.48;
/** 斜边的陡峭程度：越大折射越集中在最外面一圈 */
const CURVE = 1.9;
/** 整体放大一点，像隔着一颗凸透镜 */
const MAGNIFY = 1.1;
/** 红、蓝两色比绿色多折 / 少折这么多（色散） */
const DISPERSION = 0.07;

/** 图标底下那团雾：圆心处最浓（盖掉这么多），到这个半径（占圆点半径的比例）散尽 */
const VEIL = 0.88;
const VEIL_REACH = 0.8;

/**
 * 把 `source`（按钮窗口盖住的那块画面，圆点在它正中间）折射后画到 `canvas` 上。
 * `canvas` 的宽高要先设成圆点的实际像素大小。
 *
 * 玻璃越通透，图标越容易和底下的字搅在一起看不清。所以珠子正中间（图标所在的地方）蒙一团雾：
 * 取底下这一小块的平均色，亮的往白里推、暗的往黑里推，从圆心往外渐渐散掉 —— 中间是一小块干净的底，
 * 边缘的折射和彩边原样留着。返回这块底的明暗（true = 暗），图标据此用浅色或深色。
 */
export function drawLens(canvas: HTMLCanvasElement, source: CanvasImageSource, sourceWidth: number, sourceHeight: number): boolean | null {
  const size = canvas.width;
  const out = canvas.getContext('2d');
  if (!out || size <= 0 || sourceWidth <= 0 || sourceHeight <= 0) return null;

  // 原图读成像素
  const scratch = document.createElement('canvas');
  scratch.width = sourceWidth;
  scratch.height = sourceHeight;
  const sctx = scratch.getContext('2d', { willReadFrequently: true });
  if (!sctx) return null;
  sctx.drawImage(source, 0, 0, sourceWidth, sourceHeight);
  const src = sctx.getImageData(0, 0, sourceWidth, sourceHeight).data;

  /** 双线性取样一个通道，出了边就用最边上那个像素 */
  const sample = (x: number, y: number, channel: number): number => {
    const cx = Math.min(Math.max(x, 0), sourceWidth - 1);
    const cy = Math.min(Math.max(y, 0), sourceHeight - 1);
    const x0 = Math.floor(cx);
    const y0 = Math.floor(cy);
    const x1 = Math.min(x0 + 1, sourceWidth - 1);
    const y1 = Math.min(y0 + 1, sourceHeight - 1);
    const fx = cx - x0;
    const fy = cy - y0;
    const a = src[(y0 * sourceWidth + x0) * 4 + channel]!;
    const b = src[(y0 * sourceWidth + x1) * 4 + channel]!;
    const c = src[(y1 * sourceWidth + x0) * 4 + channel]!;
    const d = src[(y1 * sourceWidth + x1) * 4 + channel]!;
    return a + (b - a) * fx + (c + (d - c) * fx - (a + (b - a) * fx)) * fy;
  };

  const image = out.createImageData(size, size);
  const data = image.data;
  const radius = size / 2;
  const scx = sourceWidth / 2;
  const scy = sourceHeight / 2;

  // 圆点正中间那一小块的平均色
  const reach = Math.max(2, Math.round(radius * 0.5));
  const mean = [0, 0, 0];
  let count = 0;
  for (let y = Math.round(scy) - reach; y <= Math.round(scy) + reach; y += 2) {
    for (let x = Math.round(scx) - reach; x <= Math.round(scx) + reach; x += 2) {
      for (let channel = 0; channel < 3; channel++) mean[channel]! += sample(x, y, channel);
      count++;
    }
  }
  for (let channel = 0; channel < 3; channel++) mean[channel]! /= Math.max(count, 1);
  const dark = 0.2126 * mean[0]! + 0.7152 * mean[1]! + 0.0722 * mean[2]! < 128;
  // 雾的颜色：平均色再往白 / 黑推一半，和图标拉开反差
  const veil = mean.map((v) => v * 0.5 + (dark ? 18 : 255) * 0.5);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const dx = x + 0.5 - radius;
      const dy = y + 0.5 - radius;
      const dist = Math.hypot(dx, dy);
      const d = dist / radius;
      const at = (y * size + x) * 4;
      // 圆外面的由圆角裁掉，这里留一点富余免得边上露缝
      if (d > 1.04) continue;
      const k = d <= FLAT ? 0 : Math.min((d - FLAT) / (1 - FLAT), 1);
      const bend = Math.pow(k, CURVE) * BEND * radius;
      const ux = dist > 0 ? dx / dist : 0;
      const uy = dist > 0 ? dy / dist : 0;
      for (let channel = 0; channel < 3; channel++) {
        // 红光折得少、蓝光折得多
        const pull = bend * (1 + (channel - 1) * DISPERSION);
        const r = dist / MAGNIFY - pull;
        const refracted = sample(scx + ux * r, scy + uy * r, channel);
        const fog = d >= VEIL_REACH ? 0 : VEIL * Math.pow(1 - d / VEIL_REACH, 0.55);
        data[at + channel] = refracted + (veil[channel]! - refracted) * fog;
      }
      data[at + 3] = 255;
    }
  }
  out.putImageData(image, 0, 0);
  return dark;
}
