/**
 * 选区几何与坐标换算。
 *
 * 这里是整个截图功能最容易出错、又最难靠肉眼发现问题的地方，所以规格 01 §6
 * 明确要求必须有单元测试。
 *
 * # 三套坐标系，不要混
 *
 * | 坐标系 | 单位 | 原点 | 用在哪 |
 * |---|---|---|---|
 * | 虚拟桌面 | 物理像素 | 虚拟桌面左上角（可为负） | 一切跨屏计算、传给 Rust 的选区 |
 * | 窗口内 CSS | CSS 像素 | 当前遮罩窗口左上角 | 鼠标事件、DOM 布局 |
 * | 底图 | 物理像素 | 当前显示器左上角 | `<img>` 的像素映射 |
 *
 * **状态一律存虚拟桌面物理像素**，只在渲染时才转成 CSS。反过来做的话，混合
 * DPI 的多屏环境下会在每次跨屏时引入累积误差。
 */

export interface Point {
  x: number;
  y: number;
}

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** 一块遮罩窗口所覆盖的显示器几何。 */
export interface MonitorFrame {
  /** 显示器左上角在虚拟桌面上的物理坐标，可能为负。 */
  originX: number;
  originY: number;
  /** 底图的物理尺寸。 */
  width: number;
  height: number;
  /**
   * 一个 CSS 像素等于多少物理像素。
   *
   * 取值方式是实测：`底图物理宽 / window.innerWidth`。3840x2160 / 150% 的屏上
   * 实测为 1.5，和 `window.devicePixelRatio` 一致。
   *
   * 之所以宁可实测也不直接读 `devicePixelRatio`：这个比例的正确性完全取决于
   * "窗口的物理尺寸"和"CSS 视口尺寸"之间的真实关系，而窗口是我们自己按物理
   * 像素摆的（见 overlay.rs 的 `place`）。实测把这层关系直接量出来，不依赖
   * WebView 内部怎么处理 DPI，混合 DPI 多屏下也不用担心某块屏对不上。
   */
  pixelRatio: number;
}

/** 窗口内 CSS 像素 → 虚拟桌面物理像素。 */
export function cssToVirtual(point: Point, frame: MonitorFrame): Point {
  return {
    x: frame.originX + Math.round(point.x * frame.pixelRatio),
    y: frame.originY + Math.round(point.y * frame.pixelRatio),
  };
}

/** 虚拟桌面物理像素 → 窗口内 CSS 像素。 */
export function virtualToCss(point: Point, frame: MonitorFrame): Point {
  return {
    x: (point.x - frame.originX) / frame.pixelRatio,
    y: (point.y - frame.originY) / frame.pixelRatio,
  };
}

/** 物理像素长度 → CSS 像素长度。 */
export function physicalToCssLength(length: number, frame: MonitorFrame): number {
  return length / frame.pixelRatio;
}

/**
 * 由拖拽的两个对角点算出矩形。
 *
 * 往左上方向拖时 dx/dy 是负的，必须归一化成非负的宽高，否则 CSS 布局和
 * Rust 侧的裁剪都会拿到无意义的负尺寸。
 */
export function rectFromPoints(anchor: Point, cursor: Point): Rect {
  const x = Math.min(anchor.x, cursor.x);
  const y = Math.min(anchor.y, cursor.y);
  return {
    x,
    y,
    width: Math.abs(cursor.x - anchor.x),
    height: Math.abs(cursor.y - anchor.y),
  };
}

/** 显示器在虚拟桌面上占的矩形。 */
export function frameRect(frame: MonitorFrame): Rect {
  return {
    x: frame.originX,
    y: frame.originY,
    width: frame.width,
    height: frame.height,
  };
}

/**
 * 把矩形夹进边界内。
 *
 * 完全不相交时返回宽高为 0 的矩形 —— 调用方靠 {@link isUsableSelection} 判断，
 * 不用在这里抛错。
 */
export function clampRect(rect: Rect, bounds: Rect): Rect {
  const left = Math.max(rect.x, bounds.x);
  const top = Math.max(rect.y, bounds.y);
  const right = Math.min(rect.x + rect.width, bounds.x + bounds.width);
  const bottom = Math.min(rect.y + rect.height, bounds.y + bounds.height);

  if (right <= left || bottom <= top) {
    return { x: left, y: top, width: 0, height: 0 };
  }

  return { x: left, y: top, width: right - left, height: bottom - top };
}

/** 选区是否大到值得提交。1px 的选区通常是误点，不是本意。 */
export function isUsableSelection(rect: Rect): boolean {
  return rect.width >= 1 && rect.height >= 1;
}
