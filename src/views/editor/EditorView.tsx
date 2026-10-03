// 图片编辑窗口（规格 03 §7）：长截图完成后、或从截图库打开一张图时用。
// 复用截图那套标注工具，工具条固定在窗口底部；图片可滚动查看。

import '@/views/annotate/annotate.css';
import '@/views/ocr/ocr.css';
import './editor.css';

import { getCurrentWindow } from '@tauri-apps/api/window';
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { editor } from '@/lib/ipc';
import { shotUrl } from '@/lib/platform';
import type { EditorAction, EditorDoc } from '@/lib/types';
import { Spinner } from '@/ui/controls';
import { notify } from '@/ui/overlays';
import { TitleBar } from '@/ui/TitleBar';
import { AnnotationEngine } from '@/views/annotate/engine';
import { defaultToolOptions, type Tool, type ToolOptions } from '@/views/annotate/model';
import { bitmapSource } from '@/views/annotate/pixels';
import { TextEditor, useEngineVersion } from '@/views/annotate/TextEditor';
import { SubToolbar, TOOL_KEYS, Toolbar, type ActionId } from '@/views/annotate/Toolbar';

const engine = new AnnotationEngine();
const PAD = 24;
const TOO_LONG_TO_PASTE = 8000;

export default function EditorView() {
  const { t } = useTranslation();
  useEngineVersion(engine);
  const [doc, setDoc] = useState<EditorDoc | null>(null);
  const [tool, setTool] = useState<Tool | null>(null);
  const [options, setOptions] = useState<ToolOptions>(defaultToolOptions);
  const [ready, setReady] = useState(false);
  const [shown, setShown] = useState(false);
  const [busy, setBusy] = useState(false);
  const [box, setBox] = useState({ width: 800, height: 600 });
  const [view, setView] = useState({ left: 0, top: 0, width: 0, height: 0 });
  const scroller = useRef<HTMLDivElement>(null);
  const committed = useRef<HTMLCanvasElement>(null);
  const drafting = useRef<HTMLCanvasElement>(null);

  const adopt = useCallback((d: EditorDoc | null) => {
    engine.reset();
    setDoc(d);
    setReady(false);
    setShown(false);
    setTool(null);
    if (!d) return;
    engine.setClip({ x: 0, y: 0, width: d.width, height: d.height });
    fetch(shotUrl(d.imageId))
      .then((r) => r.blob())
      .then((b) => createImageBitmap(b, { colorSpaceConversion: 'none' }))
      .then((bitmap) => {
        engine.setBackdrop({ pixels: bitmapSource(bitmap), bitmap });
        setReady(true);
      })
      .catch(notify.error);
  }, []);

  useEffect(() => {
    editor.current().then(adopt).catch(notify.error);
  }, [adopt]);
  useEvent('editor-open', adopt);

  // 默认按物理像素 1:1 显示（150% 屏上就是截图时看到的样子），宽了再缩小
  const dpr = window.devicePixelRatio;
  const ds = doc ? Math.max(dpr, doc.width / Math.max(200, box.width - PAD * 2)) : dpr;
  engine.scale = dpr;
  const stageW = doc ? doc.width / ds : 0;
  const stageH = doc ? doc.height / ds : 0;

  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setBox({ width: el.clientWidth, height: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  /** 画布只覆盖可见区域：滚动时移动画布并重画（超长图整张建画布太费显存）。 */
  const syncView = useCallback(() => {
    const el = scroller.current;
    if (!el || !doc) return;
    const offsetX = Math.max(PAD, (el.clientWidth - stageW) / 2);
    const left = Math.max(0, el.scrollLeft - offsetX);
    const top = Math.max(0, el.scrollTop - PAD);
    const width = Math.min(stageW - left, el.clientWidth);
    const height = Math.min(stageH - top, el.clientHeight);
    setView({
      left,
      top,
      width: Math.max(1, width),
      height: Math.max(1, height),
    });
  }, [doc, stageW, stageH]);

  useEffect(() => syncView(), [syncView, box]);

  useLayoutEffect(() => {
    const c1 = committed.current;
    const c2 = drafting.current;
    if (!c1 || !c2) return;
    const pw = Math.ceil(view.width * ds);
    const ph = Math.ceil(view.height * ds);
    for (const c of [c1, c2]) {
      if (c.width !== pw) c.width = pw;
      if (c.height !== ph) c.height = ph;
    }
    engine.attach(c1, c2);
    engine.setOrigin({ x: Math.floor(view.left * ds), y: Math.floor(view.top * ds) });
  }, [view, ds]);

  const toPx = (e: { clientX: number; clientY: number }, stage: HTMLElement) => {
    const r = stage.getBoundingClientRect();
    return { x: Math.round((e.clientX - r.left) * ds), y: Math.round((e.clientY - r.top) * ds) };
  };

  const finish = useCallback(
    async (action: EditorAction) => {
      if (!doc || busy) return false;
      if (engine.text) engine.commitText();
      setBusy(true);
      try {
        const layer = await engine.exportLayer();
        await editor.finish({ docId: doc.id, action, annotationAt: layer?.at ?? null }, layer?.png ?? new Uint8Array());
        return true;
      } catch (err) {
        notify.error(err);
        return false;
      } finally {
        setBusy(false);
      }
    },
    [doc, busy],
  );

  /** ✓ / Enter：复制后关窗。超过 8000px 的长图很多程序粘贴不了，改为保存（规格 03 §7）。 */
  const done = useCallback(async () => {
    if (doc && doc.height > TOO_LONG_TO_PASTE) {
      notify.info(t('editor.tooLongSave'));
      void finish('save');
    } else if (await finish('copy')) {
      void getCurrentWindow().close();
    }
  }, [doc, finish, t]);

  const onAction = (id: ActionId) => {
    if (id === 'cancel') void getCurrentWindow().close();
    else if (id === 'done') void done();
    else if (id !== 'longshot') void finish(id);
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (engine.text) return;
      const ctrl = e.ctrlKey || e.metaKey;
      const k = e.key.toLowerCase();
      if (ctrl && k === 'z') return void (e.preventDefault(), engine.undo());
      if (e.key === 'Enter') return void (e.preventDefault(), done());
      if (ctrl && k === 'c') return void (e.preventDefault(), finish('copy'));
      if (ctrl && k === 's') return void (e.preventDefault(), finish('save'));
      if (ctrl && k === 'p') return void (e.preventDefault(), finish('pin'));
      if (ctrl && k === 't') return void (e.preventDefault(), finish('translate'));
      if (ctrl && k === 'w') return void (e.preventDefault(), getCurrentWindow().close());
      if (!ctrl && !e.altKey) {
        const next = (Object.keys(TOOL_KEYS) as Tool[]).find((x) => TOOL_KEYS[x] === e.key.toUpperCase());
        if (next) setTool((cur) => (cur === next ? null : next));
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [finish, done]);

  return (
    <div className="ed-root">
      <TitleBar title={doc ? `${t('editor.title')}  ·  ${doc.width} × ${doc.height}` : t('editor.title')} />
      <div className="ed-main">
        {doc && !shown && (
          <div className="ed-loading">
            <Spinner size={20} />
          </div>
        )}
        <div ref={scroller} className="ed-scroll" onScroll={syncView}>
          {doc && (
            <div
              className="ed-stage-wrap"
              style={{ width: Math.max(stageW + PAD * 2, box.width), minHeight: stageH + PAD * 2 }}
            >
              <div
                className="ed-stage"
                data-loading={!shown || undefined}
                style={{ width: stageW, height: stageH, cursor: tool === 'text' ? 'text' : tool ? 'crosshair' : 'default' }}
                onPointerDown={(e) => {
                  if (e.button !== 0 || !tool) return;
                  if (engine.text) {
                    engine.commitText();
                    return;
                  }
                  (e.target as Element).setPointerCapture(e.pointerId);
                  const p = toPx(e, e.currentTarget);
                  if (tool === 'text') engine.startText(p, options);
                  else if (tool !== 'mosaic' || ready) engine.begin(tool, p, options);
                }}
                onPointerMove={(e) => engine.drawing && engine.update(toPx(e, e.currentTarget), e.shiftKey)}
                onPointerUp={() => engine.drawing && engine.end()}
              >
                <img
                  src={shotUrl(doc.imageId)}
                  alt=""
                  draggable={false}
                  style={{ width: stageW, height: stageH }}
                  onLoad={() => setShown(true)}
                  onError={() => setShown(true)}
                />
                <canvas
                  ref={committed}
                  className="ed-canvas"
                  style={{ left: view.left, top: view.top, width: view.width, height: view.height }}
                />
                <canvas
                  ref={drafting}
                  className="ed-canvas"
                  style={{ left: view.left, top: view.top, width: view.width, height: view.height }}
                />
                <div className="ed-text-layer">
                  <TextEditor engine={engine} displayScale={ds} />
                </div>
              </div>
            </div>
          )}
        </div>
      </div>
      <footer className="ed-dock">
        {tool && <SubToolbar className="ed-sub" tool={tool} options={options} onChange={setOptions} />}
        <Toolbar
          tool={tool}
          onTool={(next) => {
            if (engine.text) engine.commitText();
            setTool(next);
          }}
          canUndo={engine.canUndo}
          onUndo={() => engine.undo()}
          actions={[
            ['ocr', 'translate', 'ai', 'pin'],
            ['save', 'cancel', 'done'],
          ]}
          onAction={onAction}
          disabledTools={{ mosaic: !ready }}
          disabledActions={{ done: busy, save: busy }}
        />
      </footer>
    </div>
  );
}
