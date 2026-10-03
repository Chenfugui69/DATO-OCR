// 放大镜（规格 02 §3.4）：120×120、6 倍（显示鼠标周围 20×20 物理像素），像素化显示，
// 中心十字线 + 中心像素描边；下方两行显示物理坐标和颜色值。原始像素到位前不渲染。

import { useLayoutEffect, useRef } from 'react';

import { formatColor } from '@/lib/format';
import type { PixelSource } from '@/views/annotate/pixels';

import { placeMagnifier, type Point } from './geometry';

const SAMPLE = 20;
const BOX = 120;
const INFO_H = 46;

export function Magnifier({
  pixels,
  cursor,
  origin,
  scale,
  viewport,
  format,
}: {
  pixels: PixelSource;
  /** 本屏局部物理坐标 */
  cursor: Point;
  /** 本屏在虚拟桌面里的原点（显示全局坐标用） */
  origin: Point;
  scale: number;
  viewport: { width: number; height: number };
  format: 'hex' | 'rgb' | 'hsl';
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const cx = Math.floor(cursor.x);
  const cy = Math.floor(cursor.y);

  useLayoutEffect(() => {
    const ctx = canvasRef.current?.getContext('2d');
    if (!ctx) return;
    const img = ctx.createImageData(SAMPLE, SAMPLE);
    const half = SAMPLE / 2;
    for (let y = 0; y < SAMPLE; y += 1) {
      for (let x = 0; x < SAMPLE; x += 1) {
        const p = pixels.get(cx - half + x, cy - half + y);
        const o = (y * SAMPLE + x) * 4;
        if (p) {
          img.data[o] = p[0];
          img.data[o + 1] = p[1];
          img.data[o + 2] = p[2];
          img.data[o + 3] = 255;
        } else {
          img.data[o + 3] = 0;
        }
      }
    }
    ctx.putImageData(img, 0, 0);
  }, [pixels, cx, cy]);

  const color = pixels.get(cx, cy) ?? [0, 0, 0];
  const pos = placeMagnifier({ x: cursor.x / scale, y: cursor.y / scale }, { width: BOX, height: BOX + INFO_H }, viewport);
  const cell = BOX / SAMPLE;

  return (
    <div className="cap-magnifier" style={{ transform: `translate(${pos.x}px, ${pos.y}px)` }}>
      <div className="cap-magnifier__view">
        <canvas ref={canvasRef} width={SAMPLE} height={SAMPLE} />
        <span className="cap-magnifier__h" style={{ top: (SAMPLE / 2) * cell, height: cell }} />
        <span className="cap-magnifier__v" style={{ left: (SAMPLE / 2) * cell, width: cell }} />
        <span className="cap-magnifier__px" style={{ left: (SAMPLE / 2) * cell, top: (SAMPLE / 2) * cell, width: cell, height: cell }} />
      </div>
      <div className="cap-magnifier__info cn-numeric">
        <div>
          ({origin.x + cx}, {origin.y + cy})
        </div>
        <div className="cap-magnifier__color">
          <span style={{ background: `rgb(${color[0]},${color[1]},${color[2]})` }} />
          {formatColor(color, format)}
        </div>
      </div>
    </div>
  );
}
