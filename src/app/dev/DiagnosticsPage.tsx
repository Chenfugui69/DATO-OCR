/**
 * M0 技术验证页。
 *
 * 这是**临时脚手架**，M6 按 06-UI设计规范.md 做真正的主窗口时会被整体替换。
 * 它存在的唯一目的是把验收清单要看的数字摆在眼前：
 *
 * - 显示器几何与缩放比例对不对（`scaleFactor` 全是 1 就说明 DPI 声明没生效）
 * - 有没有负坐标（副屏在左侧/上方时必须出现）
 * - 不依赖全局热键也能触发一次截图，便于在热键被占用时定位问题
 */

import { useCallback, useEffect, useState } from 'react';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';

import {
  captureTrigger,
  getSystemDiagnostics,
  openDataFolder,
  type SystemDiagnostics,
} from '@/lib/ipc';

export function DiagnosticsPage() {
  const [info, setInfo] = useState<SystemDiagnostics | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    getSystemDiagnostics()
      .then((next) => {
        setInfo(next);
        setError(null);
      })
      .catch((cause: unknown) => {
        setError(cause instanceof Error ? cause.message : String(cause));
      });
  }, []);

  useEffect(refresh, [refresh]);

  return (
    <div className="shell">
      <header className="titlebar" data-tauri-drag-region>
        <span data-tauri-drag-region>CHENOCR</span>
        <button
          type="button"
          className="titlebar-close"
          aria-label="收进托盘"
          onClick={() => void getCurrentWebviewWindow().hide()}
        >
          ×
        </button>
      </header>

      <main className="content">
        <h1>M0 技术验证</h1>
        <p className="subtitle">
          按 F1 触发截图。拖拽框选，Enter 复制到剪贴板，Esc 取消。
        </p>

        <div className="actions">
          <button type="button" className="primary" onClick={() => void captureTrigger()}>
            触发截图（不经热键）
          </button>
          <button type="button" className="secondary" onClick={() => void openDataFolder()}>
            打开数据目录
          </button>
          <button type="button" className="secondary" onClick={refresh}>
            刷新
          </button>
        </div>

        {error ? <p className="error">读取自检信息失败：{error}</p> : null}

        {info ? (
          <>
            <section>
              <h2>运行环境</h2>
              <dl className="facts">
                <dt>版本</dt>
                <dd>{info.appVersion}</dd>
                <dt>数据目录</dt>
                <dd>{info.dataDir}</dd>
                <dt>系统深色模式</dt>
                <dd>{info.darkMode ? '开' : '关'}</dd>
                <dt>系统透明效果</dt>
                <dd>{info.transparencyEnabled ? '开' : '关'}</dd>
                <dt>本窗口 devicePixelRatio</dt>
                <dd>{window.devicePixelRatio}</dd>
              </dl>
            </section>

            <section>
              <h2>显示器（{info.monitors.length} 块，坐标为物理像素）</h2>
              <table>
                <thead>
                  <tr>
                    <th>名称</th>
                    <th>位置</th>
                    <th>尺寸</th>
                    <th>工作区</th>
                    <th>缩放</th>
                    <th>刷新率</th>
                    <th>主屏</th>
                  </tr>
                </thead>
                <tbody>
                  {info.monitors.map((monitor) => (
                    <tr key={monitor.id}>
                      <td>{monitor.name}</td>
                      <td>
                        {monitor.bounds.x}, {monitor.bounds.y}
                      </td>
                      <td>
                        {monitor.bounds.width} × {monitor.bounds.height}
                      </td>
                      <td>
                        {monitor.workArea.width} × {monitor.workArea.height}
                      </td>
                      <td>{Math.round(monitor.scaleFactor * 100)}%</td>
                      <td>{monitor.refreshRate ? `${monitor.refreshRate} Hz` : '—'}</td>
                      <td>{monitor.isPrimary ? '是' : ''}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </section>
          </>
        ) : null}
      </main>
    </div>
  );
}
