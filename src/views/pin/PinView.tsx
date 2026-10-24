// 贴图窗口（规格 02 §6，参照 Snipaste）：
// 左键拖动 · 滚轮缩放（以鼠标为锚点，20%–500%）· Ctrl+滚轮调不透明度（20%–100%）·
// 双击收成小圆点 / 恢复 · 右键菜单 · Esc 或中键关闭。
// 图里有字的话可以直接拖选：鼠标第一次移上来时识别一遍，在图上铺一层透明文字（位置、大小和图上的字对齐），
// 在字上按下是选字，在空白处按下仍是拖动贴图。
//
// 鼠标移上来时右下角出一个半透明的小关闭按钮；图里看起来有表格 / 公式的话，关闭按钮左边再出一个一样不显眼的提示，
// 点一下：表格原位盖在图上、像 Excel 一样改（和截图里同一套），公式原位显示、能改 LaTeX。
// 右键菜单是自己画的（单独一个小窗口，见 popmenu.rs）：图标、快捷键、深浅色和程序别处一样。

import { PhysicalPosition, PhysicalSize } from '@tauri-apps/api/dpi';
import { getCurrentWindow } from '@tauri-apps/api/window';
import clsx from 'clsx';
import { Copy, FileSpreadsheet, PenLine, Sigma, Table2, X } from 'lucide-react';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useEvent } from '@/lib/events';
import { capture, ocr, pin, popmenu, system } from '@/lib/ipc';
import { isMac, modKey, shotUrl, windowLabel } from '@/lib/platform';
import type { CaptureTable, LaidCell, PinInfo, PinText, PopItem, RegionStyle } from '@/lib/types';
import { Spinner } from '@/ui/controls';
import { notify } from '@/ui/overlays';
import '@/views/capture/capture.css';
import { latexToMathml } from '@/views/capture/formulaRender';
import { TableSheet } from '@/views/capture/TableSheet';
import { splitFormula } from '@/views/ocr/StructuredResult';
import { tableGrid, toExcelText, toHtmlTable, type Cell } from '@/views/ocr/structured';

const LABEL = windowLabel();
const DOT = 24;
/** 透明文字的字号 = 框高 × 这个比例（行高留一点给上下的空白） */
const TEXT_EM = 0.86;
const TEXT_FONT = `-apple-system, 'PingFang SC', 'Segoe UI', 'Microsoft YaHei UI', sans-serif`;
const MOD = isMac ? '⌘' : 'Ctrl+';

/**
 * 当前选中的文字。各段文字是一个个绝对定位的块，浏览器拼起来时段与段之间什么都不加
 * （「第一行第二行」连成一串），这里自己拼：同一行的段之间加空格，换了行加换行。
 */
function selectedText(texts: PinText[] | null): string {
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return '';
  const range = sel.getRangeAt(0);
  const spans = [...document.querySelectorAll<HTMLElement>('.pin-text span[data-i]')];
  let out = '';
  let prev: PinText | null = null;
  for (const span of spans) {
    if (!range.intersectsNode(span)) continue;
    const box = texts?.[Number(span.dataset.i)];
    const full = span.textContent ?? '';
    const start = span.contains(range.startContainer) ? range.startOffset : 0;
    const end = span.contains(range.endContainer) ? range.endOffset : full.length;
    const part = full.slice(start, end);
    if (!part) continue;
    if (prev && box) {
      // 下一段的竖直中线落在上一段的高度范围里，就算同一行
      const mid = box.y + box.height / 2;
      out += mid > prev.y && mid < prev.y + prev.height ? ' ' : '\n';
    }
    out += part;
    prev = box ?? prev;
  }
  return out.trim() || sel.toString().trim();
}

/** 每段文字横向要拉伸多少才和图上的字一样宽：按 100px 字号量一次，跟缩放无关 */
function useTextFits(texts: PinText[] | null): number[] {
  return useMemo(() => {
    if (!texts) return [];
    const ctx = document.createElement('canvas').getContext('2d');
    if (!ctx) return texts.map(() => 1);
    ctx.font = `100px ${TEXT_FONT}`;
    return texts.map((b) => {
      const natural = (ctx.measureText(b.text).width * b.height * TEXT_EM) / 100;
      return natural > 0 ? b.width / natural : 1;
    });
  }, [texts]);
}

/** 认出来的格子 + 改过的字 → 导出用的一行行单元格 */
function toRows(cells: LaidCell[], texts: string[]): Cell[][] {
  const rows: Cell[][] = [];
  cells.forEach((c, i) => (rows[c.row] ??= []).push({ text: texts[i] ?? c.text, header: c.header, rowSpan: c.rowSpan, colSpan: c.colSpan }));
  return rows.filter(Boolean);
}

type Mode = { kind: 'table'; data: CaptureTable } | { kind: 'formula'; text: string; region: RegionStyle };

/** 贴图上原位显示的公式：铺底色，公式撑满（宽九成二、高八成），可以改 LaTeX（图底下一条输入框） */
function PinFormula({ text, region, width, height, editing }: { text: string; region: RegionStyle; width: number; height: number; editing: boolean }) {
  const parsed = useMemo(() => splitFormula(text), [text]);
  const [latex, setLatex] = useState(parsed.latex);
  const fresh = useMemo(() => latexToMathml(latex), [latex]);
  const lastGood = useRef('');
  if (fresh) lastGood.current = fresh;
  const mathml = fresh ?? (lastGood.current || parsed.mathml);
  const math = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState(24);
  useLayoutEffect(() => {
    const el = math.current;
    if (!el) return;
    el.style.fontSize = '24px';
    const r = el.getBoundingClientRect();
    if (!r.width || !r.height) return;
    const next = Math.min(200, Math.max(8, 24 * Math.min((width * 0.92) / r.width, ((editing ? height - 40 : height) * 0.8) / r.height)));
    el.style.fontSize = `${next}px`;
    setSize(next);
  }, [mathml, latex, width, height, editing]);
  // 菜单 / 按钮要复制时拿现在的 LaTeX、MathML
  useEffect(() => {
    pinFormulaNow.latex = latex;
    pinFormulaNow.mathml = mathml;
  }, [latex, mathml]);
  return (
    <div className="pin-formula" style={{ background: region.background, color: region.color }} onMouseDown={(e) => e.stopPropagation()} onDoubleClick={(e) => e.stopPropagation()}>
      {mathml ? (
        <div ref={math} className="cap-inplace__math" style={{ fontSize: size }} dangerouslySetInnerHTML={{ __html: mathml }} />
      ) : (
        <div ref={math} className="cap-inplace__math cap-inplace__math--latex" style={{ fontSize: size }}>
          {latex}
        </div>
      )}
      {editing && (
        <textarea
          className="pin-formula__latex cn-selectable"
          value={latex}
          spellCheck={false}
          autoFocus
          data-invalid={(!fresh && !!latex.trim()) || undefined}
          onChange={(e) => setLatex(e.currentTarget.value)}
          onKeyDown={(e) => e.stopPropagation()}
        />
      )}
    </div>
  );
}

/** 公式现在的 LaTeX / MathML（复制按钮用） */
const pinFormulaNow = { latex: '', mathml: '' };

export default function PinView() {
  const { t } = useTranslation();
  const [info, setInfo] = useState<PinInfo | null>(null);
  const [zoom, setZoom] = useState(1);
  const [opacity, setOpacity] = useState(1);
  const [collapsed, setCollapsed] = useState(false);
  const [onTop, setOnTop] = useState(true);
  const [closing, setClosing] = useState(false);
  const zoomRef = useRef(1);
  const [texts, setTexts] = useState<PinText[] | null>(null);
  /** 旋转 / 翻转后图变了，地址加个版本号让 img 重新取 */
  const [imageVersion, setImageVersion] = useState(0);
  const textAsked = useRef(false);
  const fits = useTextFits(texts);
  // 表格 / 公式：看起来像哪个（右下角的提示）、正在认、认出来之后的样子
  const [hint, setHint] = useState<'table' | 'formula' | null>(null);
  const [busy, setBusy] = useState(false);
  const [mode, setMode] = useState<Mode | null>(null);
  const [cellTexts, setCellTexts] = useState<string[]>([]);
  const [editingLatex, setEditingLatex] = useState(false);

  // 鼠标移上来才识别字、看是不是表格 / 公式：大多数贴图只是看一眼，不必每张都跑一遍
  const loadText = useCallback(() => {
    if (textAsked.current) return;
    textAsked.current = true;
    pin
      .text(LABEL)
      .then(setTexts)
      .catch(() => setTexts([]));
    pin
      .detectStructure(LABEL)
      .then(setHint)
      .catch(() => setHint(null));
  }, []);

  const copySelectedText = useCallback(async () => {
    const text = selectedText(texts);
    if (!text) return false;
    await capture.writeText(text);
    notify.success(t('pin.textCopied'));
    return true;
  }, [t, texts]);

  useEffect(() => {
    pin.info(LABEL).then(setInfo).catch(notify.error);
  }, []);

  const dpr = window.devicePixelRatio;

  /** 把窗口调到 zoom 下的尺寸。anchor：保持不动的点（窗口内 CSS 坐标）。 */
  const applyZoom = useCallback(
    async (next: number, anchor?: { x: number; y: number }) => {
      if (!info) return;
      const win = getCurrentWindow();
      const marginPx = Math.round(info.margin * dpr);
      const prev = zoomRef.current;
      zoomRef.current = next;
      setZoom(next);
      const w = Math.round(info.width * next) + marginPx * 2;
      const h = Math.round(info.height * next) + marginPx * 2;
      const pos = await win.outerPosition();
      let x = pos.x;
      let y = pos.y;
      if (anchor) {
        // 锚点相对图片的比例在缩放前后保持不变
        const ax = (anchor.x - info.margin) * dpr;
        const ay = (anchor.y - info.margin) * dpr;
        x = Math.round(pos.x + ax - (ax * next) / prev);
        y = Math.round(pos.y + ay - (ay * next) / prev);
      }
      await win.setSize(new PhysicalSize(w, h));
      await win.setPosition(new PhysicalPosition(x, y));
    },
    [info, dpr],
  );

  const close = useCallback(() => {
    setClosing(true);
    window.setTimeout(() => void pin.close(LABEL), 120);
  }, []);

  const toggleCollapse = useCallback(async () => {
    if (!info) return;
    const win = getCurrentWindow();
    const marginPx = Math.round(info.margin * dpr);
    if (collapsed) {
      setCollapsed(false);
      await win.setSize(new PhysicalSize(Math.round(info.width * zoomRef.current) + marginPx * 2, Math.round(info.height * zoomRef.current) + marginPx * 2));
    } else {
      setCollapsed(true);
      const s = Math.round(DOT * dpr) + marginPx * 2;
      await win.setSize(new PhysicalSize(s, s));
    }
  }, [collapsed, info, dpr]);

  // 识别二维码 / 条码：结果再弹一个菜单，点一项复制或打开
  const scanCodes = useCallback(async () => {
    const codes = await pin.scanCodes(LABEL);
    if (!codes.length) {
      void system.toast('info', t('capture.noCode'));
      return;
    }
    const items: PopItem[] = [];
    codeActions.current = {};
    codes.forEach((c, i) => {
      const short = c.text.length > 42 ? `${c.text.slice(0, 42)}…` : c.text;
      items.push({ kind: 'item', id: `code-copy-${i}`, label: `${t('pin.copyCode')}  ${short}`, icon: 'copy' });
      codeActions.current[`code-copy-${i}`] = () =>
        void capture
          .writeText(c.text)
          .then(() => system.toast('success', t('capture.codeCopied')))
          .catch(notify.error);
      if (/^https?:\/\/\S+$/i.test(c.text.trim())) {
        items.push({ kind: 'item', id: `code-open-${i}`, label: `${t('capture.openLink')}  ${short}`, icon: 'qr' });
        codeActions.current[`code-open-${i}`] = () => void system.openUrl(c.text.trim()).catch(notify.error);
      }
    });
    await popmenu.open(LABEL, items);
  }, [t]);
  const codeActions = useRef<Record<string, () => void>>({});

  // 旋转 / 翻转：Rust 那边改图，这边按新尺寸以原来的中心为准调窗口
  const transform = useCallback(
    async (op: 'rotateCw' | 'rotateCcw' | 'flipH' | 'flipV') => {
      if (!info) return;
      const next = await pin.transform(LABEL, op);
      const win = getCurrentWindow();
      const marginPx = Math.round(next.margin * dpr);
      const pos = await win.outerPosition();
      const size = await win.outerSize();
      const w = Math.round(next.width * zoomRef.current) + marginPx * 2;
      const h = Math.round(next.height * zoomRef.current) + marginPx * 2;
      setInfo(next);
      setImageVersion((v) => v + 1);
      // 识别过的字、表格的位置不对了，鼠标再移上来时重新识别
      setTexts(null);
      setMode(null);
      setHint(null);
      textAsked.current = false;
      await win.setSize(new PhysicalSize(w, h));
      await win.setPosition(new PhysicalPosition(Math.round(pos.x + (size.width - w) / 2), Math.round(pos.y + (size.height - h) / 2)));
    },
    [info, dpr],
  );

  // ── 表格 / 公式 ──
  const openTable = useCallback(async () => {
    setBusy(true);
    try {
      const data = await pin.table(LABEL);
      setCellTexts(data.table.cells.map((c) => c.text));
      setMode({ kind: 'table', data });
    } catch (err) {
      void system.toast('error', err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }, []);
  const openFormula = useCallback(async () => {
    setBusy(true);
    try {
      const [text, region] = await Promise.all([pin.structure(LABEL, 'formula'), pin.regionStyle(LABEL)]);
      setEditingLatex(false);
      setMode({ kind: 'formula', text, region });
    } catch (err) {
      void system.toast('error', err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }, []);
  const copyTable = useCallback(() => {
    if (mode?.kind !== 'table') return;
    const rows = toRows(mode.data.table.cells, cellTexts);
    void capture
      .writeRich(toExcelText(tableGrid(rows)), toHtmlTable(rows))
      .then(() => system.toast('success', t('ocr.structured.copied')))
      .catch(notify.error);
  }, [mode, cellTexts, t]);
  const exportXlsx = useCallback(() => {
    if (mode?.kind !== 'table') return;
    void ocr
      .exportXlsx(toRows(mode.data.table.cells, cellTexts))
      .then((path) => {
        if (path) void system.toast('success', t('ocr.structured.exported'));
      })
      .catch(notify.error);
  }, [mode, cellTexts, t]);
  const copyFormula = useCallback(
    (what: 'latex' | 'mathml') => {
      const value = what === 'latex' ? pinFormulaNow.latex.trim() : pinFormulaNow.mathml;
      if (!value) return;
      void capture
        .writeText(value)
        .then(() => system.toast('success', t('ocr.structured.copied')))
        .catch(notify.error);
    },
    [t],
  );

  // ── 右键菜单 ──
  const menuActions = useRef<Record<string, () => void>>({});
  /** 表格的右键菜单（TableSheet 给的）：选了哪项交回给它 */
  const sheetRun = useRef<((id: string) => void) | null>(null);
  const showMenu = useCallback(async () => {
    const win = getCurrentWindow();
    const hasText = !!selectedText(texts);
    menuActions.current = {
      copyText: () => void copySelectedText().catch(notify.error),
      copy: () => void pin.copy(LABEL).catch(notify.error),
      save: () => void pin.save(LABEL).catch(notify.error),
      ocr: () => void pin.ocr(LABEL, false).catch(notify.error),
      translate: () => void pin.ocr(LABEL, true).catch(notify.error),
      qr: () => void scanCodes().catch(notify.error),
      table: () => void openTable(),
      formula: () => void openFormula(),
      edit: () => void pin.edit(LABEL).catch(notify.error),
      rotateCw: () => void transform('rotateCw').catch(notify.error),
      rotateCcw: () => void transform('rotateCcw').catch(notify.error),
      flipH: () => void transform('flipH').catch(notify.error),
      flipV: () => void transform('flipV').catch(notify.error),
      originalSize: () => void applyZoom(1),
      opacity: () => setOpacity(1),
      // 鼠标穿透：点到贴图上等于点到它下面的窗口；从托盘菜单"取消贴图的鼠标穿透"恢复
      clickThrough: () => {
        void win.setIgnoreCursorEvents(true);
        void system.toast('info', t('pin.clickThroughHint'));
      },
      onTop: () => {
        const next = !onTop;
        setOnTop(next);
        void win.setAlwaysOnTop(next);
      },
      close,
      closeAll: () => void pin.closeAll(),
    };
    const item = (id: string, label: string, extra: Partial<Extract<PopItem, { kind: 'item' }>> = {}): PopItem => ({ kind: 'item', id, label, icon: id, ...extra });
    const sep: PopItem = { kind: 'separator' };
    const items: PopItem[] = [
      ...(hasText ? [item('copyText', t('pin.copyText'))] : []),
      item('copy', t('pin.copy'), { shortcut: `${MOD}C` }),
      item('save', t('pin.saveAs'), { shortcut: `${MOD}S` }),
      sep,
      item('ocr', t('pin.ocr')),
      item('translate', t('pin.translate')),
      item('qr', t('pin.scanCode')),
      item('table', t('actions.table')),
      item('formula', t('actions.formula')),
      item('edit', t('pin.edit')),
      {
        kind: 'row',
        label: t('pin.transform'),
        items: (['rotateCcw', 'rotateCw', 'flipH', 'flipV'] as const).map((id) => ({ id, icon: id, label: t(`pin.${id}`) })),
      },
      sep,
      item('originalSize', t('pin.originalSize'), { disabled: zoomRef.current === 1 }),
      item('opacity', t('pin.fullOpacity'), { disabled: opacity === 1 }),
      item('clickThrough', t('pin.clickThrough')),
      item('onTop', t('pin.alwaysOnTop'), { checked: onTop }),
      sep,
      item('close', t('pin.close'), { shortcut: 'Esc' }),
      item('closeAll', t('pin.closeAll'), { danger: true }),
    ];
    await popmenu.open(LABEL, items);
  }, [t, applyZoom, onTop, opacity, close, copySelectedText, texts, scanCodes, transform, openTable, openFormula]);
  useEvent('popmenu-select', (id) => {
    if (id.startsWith('sheet:')) sheetRun.current?.(id);
    else (menuActions.current[id] ?? codeActions.current[id])?.();
  });

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        // 表格 / 公式开着：先收起它们
        if (mode) setMode(null);
        else close();
      }
      // 选了字就复制字，没选就复制整张图
      else if (modKey(e) && e.key.toLowerCase() === 'c') {
        void copySelectedText()
          .then((copied) => (copied ? undefined : pin.copy(LABEL)))
          .catch(notify.error);
      } else if (modKey(e) && e.key.toLowerCase() === 's') void pin.save(LABEL).catch(notify.error);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [close, copySelectedText, mode]);

  if (!info) return null;
  const pixelated = zoom >= 2;
  const imgW = (info.width * zoom) / dpr;
  const imgH = (info.height * zoom) / dpr;
  const hintIcon = hint === 'formula' ? Sigma : Table2;
  return (
    <div
      className="pin-root"
      data-closing={closing || undefined}
      style={{ padding: info.margin }}
      onWheel={(e) => {
        if (collapsed) return;
        if (modKey(e)) {
          setOpacity((o) => Math.min(1, Math.max(0.2, Math.round((o + (e.deltaY < 0 ? 0.1 : -0.1)) * 10) / 10)));
        } else {
          const next = Math.min(5, Math.max(0.2, Math.round((zoomRef.current + (e.deltaY < 0 ? 0.1 : -0.1)) * 10) / 10));
          if (next !== zoomRef.current) void applyZoom(next, { x: e.clientX, y: e.clientY });
        }
      }}
      onMouseDown={(e) => {
        if (e.button === 1) {
          e.preventDefault();
          close();
        } else if (e.button === 0 && e.detail === 1) {
          window.getSelection()?.removeAllRanges();
          void getCurrentWindow().startDragging();
        }
      }}
      onDoubleClick={() => void toggleCollapse()}
      onContextMenu={(e) => {
        e.preventDefault();
        void showMenu();
      }}
    >
      {collapsed ? (
        <div className="pin-dot" style={{ width: DOT, height: DOT }}>
          <img src={`${shotUrl(info.imageId)}?v=${imageVersion}`} alt="" draggable={false} />
        </div>
      ) : (
        <div className="pin-frame" style={{ opacity }} onPointerEnter={loadText}>
          <img
            src={`${shotUrl(info.imageId)}?v=${imageVersion}`}
            alt=""
            draggable={false}
            // 窗口是藏着建的，图解码好了才让 Rust 显示（之后再触发也没关系，只生效一次）。
            // 不能等 requestAnimationFrame：macOS 上藏着的网页不跑动画帧，回调永远不来，每次都要等满超时 1.5 秒
            onLoad={(e) => void e.currentTarget.decode().catch(() => undefined).then(() => pin.ready(LABEL))}
            onError={() => void pin.ready(LABEL)}
            style={{ width: imgW, height: imgH, imageRendering: pixelated ? 'pixelated' : 'auto' }}
          />
          {!mode && texts && texts.length > 0 && (
            <div className="pin-text" aria-hidden={false}>
              {texts.map((b, i) => {
                const k = zoom / dpr;
                return (
                  <span
                    key={i}
                    data-i={i}
                    style={{
                      left: b.x * k,
                      top: b.y * k,
                      height: b.height * k,
                      fontSize: b.height * k * TEXT_EM,
                      lineHeight: `${b.height * k}px`,
                      transform: `scaleX(${fits[i] ?? 1})`,
                    }}
                    // 在字上按下是选字：别让外层把它当成拖动窗口；双击选词，不收成小圆点
                    onMouseDown={(e) => e.button === 0 && e.stopPropagation()}
                    onDoubleClick={(e) => e.stopPropagation()}
                  >
                    {b.text}
                  </span>
                );
              })}
            </div>
          )}
          {mode?.kind === 'table' && (
            <TableSheet
              data={mode.data}
              s={dpr / zoom}
              texts={cellTexts}
              onTexts={setCellTexts}
              style={{ left: 0, top: 0, width: imgW, height: imgH }}
              menuHost={(items, run) => {
                sheetRun.current = run;
                void popmenu.open(LABEL, items);
              }}
            />
          )}
          {mode?.kind === 'formula' && <PinFormula text={mode.text} region={mode.region} width={imgW} height={imgH} editing={editingLatex} />}
          {zoom !== 1 && <span className="pin-zoom cn-glass-thin cn-numeric">{Math.round(zoom * 100)}%</span>}
          {/* 鼠标移上来才出现：右下角关闭，左边是表格 / 公式的提示（开着时换成它们的几个操作） */}
          <div className="pin-actions" onMouseDown={(e) => e.stopPropagation()} onDoubleClick={(e) => e.stopPropagation()}>
            {mode?.kind === 'table' && (
              <>
                <PinButton icon={Copy} label={t('pin.copyTable')} onClick={copyTable} />
                <PinButton icon={FileSpreadsheet} label={t('pin.exportXlsx')} onClick={exportXlsx} />
                <PinButton icon={Table2} label={t('pin.exitTable')} on onClick={() => setMode(null)} />
              </>
            )}
            {mode?.kind === 'formula' && (
              <>
                <PinButton text="TeX" label={t('pin.copyLatex')} onClick={() => copyFormula('latex')} />
                <PinButton text="ML" label={t('pin.copyMathml')} onClick={() => copyFormula('mathml')} />
                <PinButton icon={PenLine} label={t('pin.editLatex')} on={editingLatex} onClick={() => setEditingLatex((v) => !v)} />
                <PinButton icon={Sigma} label={t('pin.exitFormula')} on onClick={() => setMode(null)} />
              </>
            )}
            {!mode && busy && (
              <span className="pin-btn" aria-label={t('pin.recognizing')}>
                <Spinner size={11} />
              </span>
            )}
            {!mode && !busy && hint && (
              <PinButton
                icon={hintIcon}
                label={hint === 'formula' ? t('pin.hintFormula') : t('pin.hintTable')}
                onClick={() => void (hint === 'formula' ? openFormula() : openTable())}
              />
            )}
            <PinButton icon={X} label={t('pin.close')} onClick={close} />
          </div>
        </div>
      )}
      <style>{`
        html[data-view='pin'], html[data-view='pin'] body { background: transparent !important; }
        /* 不做出现动画：贴图要原地、原样接替截图选区，一缩一放就成了"跳一下" */
        .pin-root { position: fixed; inset: 0; }
        .pin-root[data-closing] { animation: pin-out 120ms var(--cn-ease-in) forwards; }
        .pin-frame { position: relative; line-height: 0; border-radius: 2px; box-shadow: 0 4px 12px rgba(0,0,0,0.28), 0 0 0 0.5px rgba(0,0,0,0.25); transition: opacity 120ms; }
        .pin-frame img { display: block; }
        .pin-text { position: absolute; inset: 0; pointer-events: none; line-height: 1; }
        .pin-text span { position: absolute; white-space: pre; transform-origin: 0 0; color: transparent; font-family: ${TEXT_FONT}; cursor: text; pointer-events: auto; user-select: text; -webkit-user-select: text; }
        .pin-text span::selection { background: rgba(10, 132, 255, 0.32); color: transparent; }
        .pin-frame:hover { box-shadow: 0 4px 12px rgba(0,0,0,0.28), 0 0 0 1px var(--cn-accent); }
        .pin-zoom { position: absolute; left: 6px; bottom: 6px; padding: 2px 6px; border-radius: 4px; font: 500 11px/14px var(--cn-font-sans); color: var(--cn-label); line-height: 14px; pointer-events: none; }
        .pin-dot { border-radius: 50%; overflow: hidden; box-shadow: 0 2px 8px rgba(0,0,0,0.35), 0 0 0 2px #fff; }
        .pin-dot img { width: 100%; height: 100%; object-fit: cover; }
        @keyframes pin-out { to { opacity: 0; transform: scale(0.94); } }
        /* 右下角的小按钮：半透明、不显眼，鼠标在贴图上才出现 */
        .pin-actions { position: absolute; right: 5px; bottom: 5px; z-index: 6; display: flex; gap: 4px; line-height: 1; opacity: 0; pointer-events: none; transition: opacity 140ms var(--cn-ease-out); }
        .pin-frame:hover .pin-actions { opacity: 1; pointer-events: auto; }
        .pin-btn { display: inline-grid; place-items: center; min-width: 20px; height: 20px; padding: 0 4px; border: 0; border-radius: 10px; background: rgba(0,0,0,0.38); color: rgba(255,255,255,0.88); font: 600 9.5px/1 var(--cn-font-sans); cursor: pointer; opacity: 0.72; transition: opacity 120ms, background 120ms; }
        .pin-btn:hover { opacity: 1; background: rgba(0,0,0,0.62); }
        .pin-btn[data-on] { opacity: 1; background: var(--cn-accent); }
        .pin-formula { position: absolute; inset: 0; display: grid; place-items: center; overflow: hidden; line-height: normal; }
        .pin-formula__latex { position: absolute; left: 4px; right: 4px; bottom: 4px; height: 34px; padding: 4px 6px; border: 0; border-radius: 6px; resize: none; outline: none; font: 12px/1.4 'Cascadia Mono', Consolas, ui-monospace, monospace; color: var(--cn-label); background: var(--cn-bg-elevated); box-shadow: 0 0 0 1.5px var(--cn-accent); }
        .pin-formula__latex[data-invalid] { color: var(--cn-red); }
      `}</style>
    </div>
  );
}

function PinButton({ icon: I, text, label, on, onClick }: { icon?: typeof X; text?: string; label: string; on?: boolean; onClick: () => void }) {
  return (
    <button type="button" className={clsx('pin-btn')} data-on={on || undefined} title={label} aria-label={label} onClick={onClick}>
      {I ? <I size={12} strokeWidth={2.2} /> : text}
    </button>
  );
}
