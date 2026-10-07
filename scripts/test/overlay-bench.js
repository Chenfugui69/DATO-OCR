// 截图遮罩的拖动基准：在遮罩页面里合成指针事件，让选区在整块屏里来回拖，统计帧间隔。
// 由 mac-overlay-bench.sh 通过调试版的 `--eval=capture-<屏号>:<本文件>` 注入执行，
// 结果用 report_error 写进应用日志（scope=bench）。
//
// 可选的 window.__BENCH__：
//   frames   拖多少帧（默认 300）
//   mode     'hover' = 不按键只移动（自动检测阶段）；默认是按住拖选
//   css      临时加一段样式，用来逐项排查，比如 '.cap-magnifier{display:none!important}'
//   noClear  true = 让画布的 clearRect 什么都不做
(async () => {
  const report = (o) => window.__TAURI_INTERNALS__.invoke('report_error', { scope: 'bench', message: JSON.stringify(o) });
  try {
    const root = document.querySelector('.cap-root');
    const P = window.__BENCH__ || {};
    // 合成事件用的指针号不一定是"活动指针"，捕获会抛异常
    Element.prototype.setPointerCapture = function () {};
    // 遮罩页面是常驻的：上一轮留下的样式和补丁先清干净，否则各轮互相污染
    document.querySelectorAll('[data-bench]').forEach((e) => e.remove());
    window.__origClearRect = window.__origClearRect || CanvasRenderingContext2D.prototype.clearRect;
    CanvasRenderingContext2D.prototype.clearRect = P.noClear ? function () {} : window.__origClearRect;
    if (P.css) {
      const st = document.createElement('style');
      st.dataset.bench = '1';
      st.textContent = P.css;
      document.head.appendChild(st);
    }
    const fire = (type, x, y, buttons) =>
      root.dispatchEvent(
        new PointerEvent(type, { bubbles: true, cancelable: true, clientX: x, clientY: y, button: type === 'pointermove' ? -1 : 0, buttons, pointerId: 1, pointerType: 'mouse', isPrimary: true }),
      );
    const frame = () => new Promise((r) => requestAnimationFrame(r));
    const W = innerWidth;
    const H = innerHeight;
    const N = P.frames || 300;
    const hover = P.mode === 'hover';
    for (let i = 0; i < 10; i++) await frame();
    if (!hover) fire('pointerdown', 60, 60, 1);
    await report({ mark: 'start' });
    const dt = [];
    let last = performance.now();
    for (let i = 0; i < N; i++) {
      const t = 0.5 - 0.5 * Math.cos((i / 90) * Math.PI);
      fire('pointermove', 80 + t * (W - 140), 80 + t * (H - 140), hover ? 0 : 1);
      await frame();
      const now = performance.now();
      dt.push(now - last);
      last = now;
    }
    // 不松开鼠标：松开会进编辑态，识字模式下还会直接出结果。由脚本用 --action=cancel 收场
    const sorted = [...dt].sort((a, b) => a - b);
    const r1 = (v) => Math.round(v * 10) / 10;
    await report({ mark: 'end', n: N, avg: r1(dt.reduce((a, b) => a + b, 0) / N), p95: r1(sorted[Math.floor(N * 0.95)]), max: r1(sorted[N - 1]), slow: dt.filter((v) => v > 20).length });
  } catch (e) {
    report({ error: String((e && e.stack) || e) });
  }
})();
