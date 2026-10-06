// 图片编辑窗口（规格 03 §7）：长截图完成后、或从截图库打开一张图时用。
// 复用截图那套标注工具，工具条固定在窗口底部；图片可滚动查看。

import '@/views/annotate/annotate.css';
import '@/views/ocr/ocr.css';
import './editor.css';

import { getCurrentWindow } from '@tauri-apps/api/window';
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { useElementSize } from '@/lib/hooks';
import { editor } from '@/lib/ipc';
import { shotUrl } from '@/lib/platform';
import type { EditorAction, EditorDoc } from '@/lib/types';
import { Spinner } from '@/ui/controls';
import { notify } from '@/ui/overlays';
import { TitleBar } from '@/ui/TitleBar';
import { AnnotationEngine, toolOf } from '@/views/annotate/engine';
import { BRUSH_RANGE, defaultToolOptions, PEN_RANGE, type Tool, type ToolOptions } from '@/views/annotate/model';
import { bitmapSource } from '@/views/annotate/pixels';
import { canPick, handleCursor, SelectionOverlay, toolCursor } from '@/views/annotate/SelectionOverlay';
import { TextEditor, useEngineVersion } from '@/views/annotate/TextEditor';
import { SubToolbar, TOOL_KEYS, Toolbar, useToolAnchor, type ActionId } from '@/views/annotate/Toolbar';

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
  /** 悬停时的指针；'tool' = 当前工具自己的指针 */
  const [hoverCursor, setHoverCursor] = useState('tool');
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

  // 选中了已有标注：二级工具条显示它的样式，改了直接作用到它身上
  const picked = engine.selectedAnnotation;
  const pickedTool = picked ? toolOf(picked) : null;
  // 二级工具条在主工具条上方居中；往对应的工具按钮那边挪，但不超出主工具条的两头
  const barRef = useRef<HTMLDivElement>(null);
  const subRef = useRef<HTMLDivElement>(null);
  const barSize = useElementSize(barRef, { width: 600, height: 44 });
  const subSize = useElementSize(subRef, { width: 300, height: 40 }, [!!(pickedTool ?? tool)]);
  const anchor = useToolAnchor(barRef, pickedTool ?? tool);
  const room = Math.max(0, (barSize.width - subSize.width) / 2);
  const subShift = anchor == null ? 0 : Math.max(-room, Math.min(room, anchor - barSize.width / 2));

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
    else if (id !== 'longshot' && id !== 'gif') void finish(id);
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (engine.text) return;
      const ctrl = e.ctrlKey || e.metaKey;
      const k = e.key.toLowerCase();
      if ((e.key === 'Delete' || e.key === 'Backspace') && engine.selected) return void (e.preventDefault(), engine.deleteSelected());
      if (e.key === 'Escape' && engine.selected) return void (e.preventDefault(), engine.select(null));
      if (ctrl && k === 'z') return void (e.preventDefault(), engine.undo());
      if (e.key === 'Enter') return void (e.preventDefault(), done());
      if (ctrl && k === 'c') return void (e.preventDefault(), finish('copy'));
      if (ctrl && k === 's') return void (e.preventDefault(), finish('save'));
      if (ctrl && k === 'p') return void (e.preventDefault(), finish('pin'));
      if (ctrl && k === 't') return void (e.preventDefault(), finish('translate'));
      if (ctrl && k === 'w') return void (e.preventDefault(), getCurrentWindow().close());
      if (!ctrl && !e.altKey) {
        const next = (Object.keys(TOOL_KEYS) as Tool[]).find((x) => TOOL_KEYS[x] === e.key.toUpperCase());
        if (next) {
          engine.select(null);
          setTool((cur) => (cur === next ? null : next));
        }
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
                style={{ width: stageW, height: stageH, cursor: hoverCursor === 'tool' ? toolCursor(tool, options) : hoverCursor }}
                onPointerDown={(e) => {
                  if (e.button !== 0) return;
                  if (engine.text) {
                    engine.commitText();
                    return;
                  }
                  (e.target as Element).setPointerCapture(e.pointerId);
                  const p = toPx(e, e.currentTarget);
                  // 先看是不是点在已画好的标注上：拖控制点改大小、拖本体挪位置
                  const ah = engine.hitHandle(p, 7 * ds);
                  if (ah) return void engine.beginTransform(p, ah);
                  const hit = canPick(tool, options) ? engine.hit(p, 4 * ds) : null;
                  // 单击已有标注（包括文字）= 选中，可拖动、拖控制点缩放；双击文字才进入编辑
                  if (hit) {
                    engine.select(hit.id);
                    engine.beginTransform(p, 'move');
                    return;
                  }
                  engine.select(null);
                  if (!tool) return;
                  if (tool === 'text') engine.startText(p, options);
                  else if (tool !== 'mosaic' || ready) engine.begin(tool, p, options);
                }}
                onPointerMove={(e) => {
                  const p = toPx(e, e.currentTarget);
                  if (engine.drawing) return engine.update(p, e.shiftKey);
                  if (engine.transforming) return engine.updateTransform(p, e.shiftKey);
                  const ah = engine.hitHandle(p, 7 * ds);
                  const next = ah ? handleCursor(ah) : canPick(tool, options) && engine.hit(p, 4 * ds) ? 'move' : 'tool';
                  if (next !== hoverCursor) setHoverCursor(next);
                }}
                onPointerUp={() => {
                  if (engine.drawing) engine.end();
                  if (engine.transforming) engine.endTransform();
                }}
                onDoubleClick={(e) => {
                  const hit = engine.hit(toPx(e, e.currentTarget), 4 * ds);
                  if (hit?.kind === 'text') engine.editText(hit.id);
                }}
                onWheel={(e) => {
                  // 画笔 / 手绘马赛克时滚轮调粗细（按住 Ctrl 才是原来的滚动缩放之类）
                  if (e.ctrlKey || (tool !== 'pen' && !(tool === 'mosaic' && options.mosaic.shape === 'brush'))) return;
                  const dir = e.deltaY < 0 ? 1 : -1;
                  const clamp = (v: number, [lo, hi]: [number, number]) => Math.max(lo, Math.min(hi, v));
                  setOptions((o) =>
                    tool === 'pen'
                      ? { ...o, pen: { ...o.pen, lineWidth: clamp(o.pen.lineWidth + dir, PEN_RANGE) } }
                      : { ...o, mosaic: { ...o.mosaic, brushSize: clamp(o.mosaic.brushSize + dir * 4, BRUSH_RANGE) } },
                  );
                }}
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
                  <SelectionOverlay engine={engine} displayScale={ds} />
                  <TextEditor engine={engine} displayScale={ds} />
                </div>
              </div>
            </div>
          )}
        </div>
      </div>
      <footer className="ed-dock">
        {(pickedTool ?? tool) && (
          <SubToolbar
            ref={subRef}
            style={{ transform: `translateX(${subShift}px)` }}
            className="ed-sub"
            tool={(pickedTool ?? tool)!}
            options={picked && pickedTool ? engine.optionsOf(picked, options) : options}
            onChange={(o) => {
              if (picked && pickedTool) engine.applyOptions(o);
              setOptions(o);
            }}
          />
        )}
        <Toolbar
          ref={barRef}
          tool={tool}
          onTool={(next) => {
            if (engine.text) engine.commitText();
            engine.select(null);
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
