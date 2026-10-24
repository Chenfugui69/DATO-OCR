// 设置页：截图工具条的按钮顺序和按钮组。画一条和真工具条一样的条（放不下就换行，不裁掉），下面是按钮组区：
// - 按住图标拖：拖着的图标跟着指针走，旁边的图标滑开让位（在条上只能在自己那一组里换：绘制工具 / 中间那组 / 最右边那组）
// - 往下拖到「新建按钮组」那块：这个按钮就变成一个新的按钮组（组里先只有它），并展开这个组
// - 点一下按钮组（右下角带小三角的）：下面展开它的组区域；把条上的按钮拖进去就入组，从组里拖回条上就移出。
//   组里拖空了，这个组就没了
// - 点一下普通图标：隐藏 / 显示它（完成、取消不能藏）
// - 右键还留着菜单（放进按钮组 / 新建 / 移出 / 改名 / 解散），点组区域里的组名也能改名
//
// 不用 HTML5 的拖放：Tauri 窗口默认接管了拖放（给拖文件进来用的），页面里的 draggable 不工作。
// 这里自己跟指针：拖起来时复制一个图标浮在最上面跟着指针，原位置留一个虚线空位；
// 空位按指针离哪个图标的中心最近挪到它前面或后面（可以跨到组区域里），松手时浮着的图标飞回空位，保存。

import '@/views/annotate/annotate.css';

import { FolderPlus, RotateCcw } from 'lucide-react';
import { Fragment, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { DEFAULT_GROUPS, MID_ACTIONS, TAIL_ACTIONS, TOOL_ORDER, type CaptureSettings } from '@/lib/types';
import { Button } from '@/ui/controls';
import { popupMenu } from '@/ui/nativeMenu';
import { promptDialog } from '@/ui/overlays';
import { GROUP_PREFIX, groupIcon, groupLabel, itemIcon, type ToolbarGroup } from '@/views/annotate/Toolbar';

type Order = Pick<CaptureSettings, 'toolbarTools' | 'toolbarActions' | 'toolbarOrder' | 'toolbarHidden' | 'toolbarGroups'>;
type ListKey = 'toolbarTools' | 'toolbarActions' | 'toolbarOrder';
const LISTS: ListKey[] = ['toolbarTools', 'toolbarActions', 'toolbarOrder'];
/** 这两个不能藏：没有它们就完不成 / 退不出截图 */
const ALWAYS_SHOWN = ['done', 'cancel'];
const DEFAULTS = { toolbarTools: TOOL_ORDER as string[], toolbarActions: MID_ACTIONS, toolbarOrder: TAIL_ACTIONS as string[] };
const EASE = 'cubic-bezier(0.22, 1, 0.36, 1)';

type Full = { toolbarTools: string[]; toolbarActions: string[]; toolbarOrder: string[]; toolbarHidden: string[]; toolbarGroups: ToolbarGroup[] };
/** 一个按钮能待的地方：条上的三组之一，或某个按钮组里 */
type Spot = ListKey | { group: string };
const spotId = (s: Spot) => (typeof s === 'string' ? s : `group:${s.group}`);

function filled(o: Order): Full {
  return {
    toolbarTools: o.toolbarTools?.length ? o.toolbarTools : DEFAULTS.toolbarTools,
    toolbarActions: o.toolbarActions?.length ? o.toolbarActions : DEFAULTS.toolbarActions,
    toolbarOrder: o.toolbarOrder?.length ? o.toolbarOrder : DEFAULTS.toolbarOrder,
    toolbarHidden: o.toolbarHidden ?? [],
    toolbarGroups: o.toolbarGroups ?? DEFAULT_GROUPS,
  };
}

const listOf = (o: Full, entry: string): ListKey | null => LISTS.find((l) => o[l].includes(entry)) ?? null;
const itemsOf = (o: Full, s: Spot): string[] => (typeof s === 'string' ? o[s] : (o.toolbarGroups.find((g) => g.id === s.group)?.items ?? []));
const withItems = (o: Full, s: Spot, items: string[]): Full =>
  typeof s === 'string' ? { ...o, [s]: items } : { ...o, toolbarGroups: o.toolbarGroups.map((g) => (g.id === s.group ? { ...g, items } : g)) };
/** 按钮现在在哪 */
function spotOf(o: Full, id: string): Spot | null {
  const l = listOf(o, id);
  if (l) return l;
  const g = o.toolbarGroups.find((x) => x.items.includes(id));
  return g ? { group: g.id } : null;
}
/** 按钮属于条上的哪一组（在按钮组里的，算那个组所在的那一组） */
function homeOf(o: Full, id: string): ListKey | null {
  const s = spotOf(o, id);
  if (!s) return null;
  return typeof s === 'string' ? s : listOf(o, GROUP_PREFIX + s.group);
}
/** 能不能进按钮组：按钮组本身不能套娃，最右边那组（完成、取消……）不进组 */
const groupable = (o: Full, id: string) => !id.startsWith(GROUP_PREFIX) && !!homeOf(o, id) && homeOf(o, id) !== 'toolbarOrder';
/** 能不能进这个组：只收和组在条上同一组的按钮（绘制工具进绘制工具的组） */
const canJoin = (o: Full, id: string, groupId: string) => groupable(o, id) && homeOf(o, id) === listOf(o, GROUP_PREFIX + groupId);

/** 把按钮从原来的地方拿走，放到 `to` 的第 `index` 个 */
function moveTo(o: Full, id: string, to: Spot, index: number): Full {
  const from = spotOf(o, id);
  const n = from ? withItems(o, from, itemsOf(o, from).filter((x) => x !== id)) : o;
  const items = [...itemsOf(n, to)];
  items.splice(Math.max(0, Math.min(index, items.length)), 0, id);
  return withItems(n, to, items);
}

/** 新建一个只有这个按钮的组：组占按钮原来的位置；按钮原来在别的组里，新组放在那个组后面 */
function makeGroup(o: Full, id: string, name = ''): [Full, string] {
  const gid = `g${Date.now().toString(36)}`;
  const ref = GROUP_PREFIX + gid;
  const group: ToolbarGroup = { id: gid, name, items: [id] };
  const l = listOf(o, id);
  if (l) return [{ ...o, [l]: o[l].map((x) => (x === id ? ref : x)), toolbarGroups: [...o.toolbarGroups, group] }, gid];
  const old = o.toolbarGroups.find((g) => g.items.includes(id));
  const home = old && listOf(o, GROUP_PREFIX + old.id);
  if (!old || !home) return [o, ''];
  const list = [...o[home]];
  list.splice(list.indexOf(GROUP_PREFIX + old.id) + 1, 0, ref);
  return [
    {
      ...o,
      [home]: list,
      toolbarGroups: [...o.toolbarGroups.map((g) => (g.id === old.id ? { ...g, items: g.items.filter((x) => x !== id) } : g)), group],
    },
    gid,
  ];
}

/** 拖空了的组连同条上的入口一起去掉 */
function prune(o: Full): Full {
  const empty = new Set(o.toolbarGroups.filter((g) => !g.items.length).map((g) => GROUP_PREFIX + g.id));
  if (!empty.size) return o;
  const keep = (l: string[]) => l.filter((x) => !empty.has(x));
  return {
    ...o,
    toolbarTools: keep(o.toolbarTools),
    toolbarActions: keep(o.toolbarActions),
    toolbarOrder: keep(o.toolbarOrder),
    toolbarGroups: o.toolbarGroups.filter((g) => g.items.length),
  };
}

/** 一串里拖着的那个按指针位置该排到第几个（离指针最近的那个图标，按左右半边决定放它前面还是后面） */
function targetIndex(items: HTMLElement[], dragged: string, x: number, y: number): number {
  let best = -1;
  let bestDist = Infinity;
  let after = false;
  items.forEach((el, i) => {
    const r = el.getBoundingClientRect();
    const cx = r.left + r.width / 2;
    const cy = r.top + r.height / 2;
    // 换行时上下离得远的不算：竖直方向的距离加重
    const d = Math.abs(x - cx) + Math.abs(y - cy) * 3;
    if (d < bestDist) {
      bestDist = d;
      best = i;
      after = x > cx;
    }
  });
  if (best < 0) return 0;
  const ids = items.map((el) => el.dataset.id);
  const from = ids.indexOf(dragged);
  let to = best + (after ? 1 : 0);
  if (from >= 0 && from < to) to -= 1;
  return to;
}

/** 浮在最上面跟着指针的那个图标（复制一份，原位置留空位） */
function makeGhost(el: HTMLElement, r: DOMRect): HTMLElement {
  const g = el.cloneNode(true) as HTMLElement;
  for (const a of ['data-id', 'data-key', 'data-list', 'data-dragging', 'data-open']) g.removeAttribute(a);
  g.classList.add('tb-order__ghost');
  Object.assign(g.style, {
    position: 'fixed',
    left: '0',
    top: '0',
    width: `${r.width}px`,
    height: `${r.height}px`,
    margin: '0',
    transition: 'none',
    transform: `translate(${r.left}px, ${r.top}px)`,
    pointerEvents: 'none',
    zIndex: '1000',
  });
  document.body.appendChild(g);
  return g;
}

export function ToolbarOrder({ value, onChange }: { value: Order; onChange: (next: Order) => void }) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<Full | null>(null);
  const [dragging, setDragging] = useState<string | null>(null);
  /** 拖着的按钮能放进哪里、指针正停在哪块上 */
  const [drop, setDrop] = useState<{ canNew: boolean; canJoin: boolean; over: 'new' | 'group' | null } | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const barRef = useRef<HTMLDivElement>(null);
  const groupBoxRef = useRef<HTMLDivElement>(null);
  const groupRowRef = useRef<HTMLDivElement>(null);
  const newZoneRef = useRef<HTMLDivElement>(null);
  const ghostRef = useRef<HTMLElement | null>(null);
  const order = draft ?? filled(value);
  // 最右边那组的顺序在类型上是固定的那几个按钮，这里统一当字符串处理，存回去时原样交出去
  const emit = (o: Full) => onChange(o as unknown as Order);
  const hidden = order.toolbarHidden;
  const groups = order.toolbarGroups;
  const open = expanded ? groups.find((g) => g.id === expanded) : undefined;
  const isDefault =
    hidden.length === 0 &&
    LISTS.every((g) => order[g].length === DEFAULTS[g].length && order[g].every((id, i) => id === DEFAULTS[g][i])) &&
    JSON.stringify(groups) === JSON.stringify(DEFAULT_GROUPS);
  const groupOf = (entry: string) => groups.find((g) => GROUP_PREFIX + g.id === entry);
  const labelOf = (id: string) => {
    const g = groupOf(id);
    if (g) return groupLabel(g, t);
    return (TOOL_ORDER as string[]).includes(id) || id === 'undo' ? t(`tools.${id}`) : t(`actions.${id}`);
  };
  const iconOf = (id: string) => {
    const g = groupOf(id);
    return g ? groupIcon(g) : itemIcon(id);
  };

  // 页面在拖动途中被关掉：别把浮着的图标留在那儿
  useEffect(() => () => ghostRef.current?.remove(), []);

  // 换位置的动画（FLIP）：重排之前记下每个图标在哪，重排之后先放回原处、再滑到新位置
  const before = useRef(new Map<string, DOMRect>());
  const remember = () => {
    before.current.clear();
    rootRef.current?.querySelectorAll<HTMLElement>('[data-id]').forEach((el) => before.current.set(el.dataset.key!, el.getBoundingClientRect()));
  };
  useLayoutEffect(() => {
    if (!before.current.size) return;
    rootRef.current?.querySelectorAll<HTMLElement>('[data-id]').forEach((el) => {
      if (el.dataset.dragging) return;
      const old = before.current.get(el.dataset.key!);
      if (!old) return;
      const now = el.getBoundingClientRect();
      const dx = old.left - now.left;
      const dy = old.top - now.top;
      if (!dx && !dy) return;
      el.style.transition = 'none';
      el.style.transform = `translate(${dx}px, ${dy}px)`;
      void el.offsetWidth;
      el.style.transition = `transform 160ms ${EASE}`;
      el.style.transform = '';
    });
    before.current.clear();
  });

  const onPointerDown = (e: React.PointerEvent<HTMLButtonElement>, id: string) => {
    if (e.button !== 0 || ghostRef.current) return;
    e.preventDefault();
    const el = e.currentTarget;
    const start = { x: e.clientX, y: e.clientY };
    const r0 = el.getBoundingClientRect();
    const grab = { x: start.x - r0.left, y: start.y - r0.top };
    const openId = open?.id ?? null;
    let current: Full = order;
    let moved = false;
    let over: 'new' | 'group' | null = null;
    const canNew = groupable(order, id);
    const canIn = !!openId && canJoin(order, id, openId);
    // 拖起来时在哪：拖到「新建组」上就先回到这里（斜着往下拖时顺路换了位置的不算），新组占它原来的位置
    const spot0 = spotOf(order, id);
    const index0 = spot0 ? itemsOf(order, spot0).indexOf(id) : -1;

    const inside = (box: HTMLElement | null, x: number, y: number, pad: number) => {
      if (!box) return false;
      const r = box.getBoundingClientRect();
      return x >= r.left - pad && x <= r.right + pad && y >= r.top - pad && y <= r.bottom + pad;
    };
    /** 指针下面是哪块：新建组那块 / 展开的组 / 条上它自己那一组；都不是就原地不动 */
    const hit = (x: number, y: number): Spot | 'new' | null => {
      if (canNew && inside(newZoneRef.current, x, y, 6)) return 'new';
      if (canIn && openId && inside(groupBoxRef.current, x, y, 6)) return { group: openId };
      if (inside(barRef.current, x, y, 28)) return homeOf(current, id);
      return null;
    };

    const move = (ev: PointerEvent) => {
      if (!moved && Math.hypot(ev.clientX - start.x, ev.clientY - start.y) < 4) return;
      if (!moved) {
        moved = true;
        ghostRef.current = makeGhost(el, r0);
        setDraft(current);
        setDragging(id);
        setDrop({ canNew, canJoin: canIn, over: null });
      }
      const ghost = ghostRef.current;
      if (ghost) ghost.style.transform = `translate(${ev.clientX - grab.x}px, ${ev.clientY - grab.y}px) scale(1.12)`;

      const zone = hit(ev.clientX, ev.clientY);
      const nowOver = zone === 'new' ? 'new' : zone && typeof zone !== 'string' ? 'group' : null;
      if (nowOver !== over) {
        over = nowOver;
        setDrop({ canNew, canJoin: canIn, over });
      }
      if (zone === 'new') {
        const now = spotOf(current, id);
        if (spot0 && (!now || spotId(now) !== spotId(spot0) || itemsOf(current, spot0).indexOf(id) !== index0)) {
          current = moveTo(current, id, spot0, index0);
          remember();
          setDraft(current);
        }
        return;
      }
      if (!zone) return;
      const root = typeof zone === 'string' ? barRef.current : groupRowRef.current;
      const items = [...(root?.querySelectorAll<HTMLElement>(`[data-list="${spotId(zone)}"]`) ?? [])];
      const to = targetIndex(items, id, ev.clientX, ev.clientY);
      const from = spotOf(current, id);
      if (from && spotId(from) === spotId(zone) && itemsOf(current, zone).indexOf(id) === to) return;
      current = moveTo(current, id, zone, to);
      remember();
      setDraft(current);
    };

    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      window.removeEventListener('pointercancel', up);
      if (!moved) {
        // 没拖、只是点了一下：按钮组 → 展开 / 收起；别的 → 切换藏不藏
        if (id.startsWith(GROUP_PREFIX)) setExpanded((x) => (x === id.slice(GROUP_PREFIX.length) ? null : id.slice(GROUP_PREFIX.length)));
        else if (!ALWAYS_SHOWN.includes(id)) emit({ ...current, toolbarHidden: hidden.includes(id) ? hidden.filter((x) => x !== id) : [...hidden, id] });
        return;
      }
      let final = current;
      let newGid = '';
      if (over === 'new') [final, newGid] = makeGroup(current, id);
      final = prune(final);
      remember();
      setDrop(null);
      setDraft(null);
      emit(final);
      if (newGid) setExpanded(newGid);
      else if (openId && !final.toolbarGroups.some((g) => g.id === openId)) setExpanded(null);
      // 浮着的图标飞回它的新位置，落下后空位才变回图标
      requestAnimationFrame(() => {
        const ghost = ghostRef.current;
        const land = rootRef.current?.querySelector<HTMLElement>('[data-id][data-dragging]');
        const done = () => {
          ghost?.remove();
          ghostRef.current = null;
          setDragging(null);
        };
        if (!ghost || !land) return done();
        const r = land.getBoundingClientRect();
        ghost.style.transition = `transform 180ms ${EASE}`;
        ghost.style.transform = `translate(${r.left}px, ${r.top}px)`;
        window.setTimeout(done, 190);
      });
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
    window.addEventListener('pointercancel', up);
  };

  // ── 右键菜单里的增删改 ──
  const putIntoGroup = (id: string, groupId: string) => emit(prune(moveTo(order, id, { group: groupId }, Infinity)));
  const newGroup = async (id: string) => {
    const name = await promptDialog({ title: t('settings.toolbarGroups.newTitle'), placeholder: t('settings.toolbarGroups.namePlaceholder') });
    if (name === null) return;
    const [next, gid] = makeGroup(order, id, name.trim());
    if (!gid) return;
    emit(prune(next));
    setExpanded(gid);
  };
  const takeOut = (id: string, groupId: string) => {
    const home = listOf(order, GROUP_PREFIX + groupId);
    if (!home) return;
    // 拿出来的按钮放在组的后面；组空了就连组一起去掉
    const next = prune(moveTo(order, id, home, order[home].indexOf(GROUP_PREFIX + groupId) + 1));
    emit(next);
    if (!next.toolbarGroups.some((g) => g.id === groupId)) setExpanded(null);
  };
  const dissolve = (groupId: string) => {
    const o = order;
    const ref = GROUP_PREFIX + groupId;
    const l = listOf(o, ref);
    const g = o.toolbarGroups.find((x) => x.id === groupId);
    if (!l || !g) return;
    emit({ ...o, [l]: o[l].flatMap((x) => (x === ref ? g.items : [x])), toolbarGroups: o.toolbarGroups.filter((x) => x.id !== groupId) });
    setExpanded(null);
  };
  const rename = async (groupId: string) => {
    const g = order.toolbarGroups.find((x) => x.id === groupId);
    if (!g) return;
    const name = await promptDialog({ title: t('settings.toolbarGroups.renameTitle'), initial: groupLabel(g, t), placeholder: t('settings.toolbarGroups.namePlaceholder') });
    if (name === null) return;
    emit({ ...order, toolbarGroups: order.toolbarGroups.map((x) => (x.id === groupId ? { ...x, name: name.trim() } : x)) });
  };
  const contextMenu = (e: React.MouseEvent, id: string, inGroup?: string) => {
    e.preventDefault();
    if (inGroup) {
      void popupMenu([{ label: t('settings.toolbarGroups.takeOut'), onSelect: () => takeOut(id, inGroup) }]);
      return;
    }
    if (id.startsWith(GROUP_PREFIX)) {
      const gid = id.slice(GROUP_PREFIX.length);
      void popupMenu([
        { label: t('settings.toolbarGroups.rename'), onSelect: () => void rename(gid) },
        { label: t('settings.toolbarGroups.dissolve'), onSelect: () => dissolve(gid) },
      ]);
      return;
    }
    if (!groupable(order, id)) return;
    const candidates = groups.filter((g) => canJoin(order, id, g.id));
    void popupMenu([
      ...(candidates.length
        ? [{ label: t('settings.toolbarGroups.putInto'), submenu: candidates.map((g) => ({ label: groupLabel(g, t), onSelect: () => putIntoGroup(id, g.id) })) }]
        : []),
      { label: t('settings.toolbarGroups.newGroup'), onSelect: () => void newGroup(id) },
    ]);
  };

  const button = (id: string, spot: Spot) => {
    const Icon = iconOf(id);
    const label = labelOf(id);
    const listId = spotId(spot);
    const inGroup = typeof spot === 'string' ? undefined : spot.group;
    return (
      <button
        key={id}
        type="button"
        data-id={id}
        data-key={`${listId}|${id}`}
        data-list={listId}
        data-dragging={dragging === id || undefined}
        data-hidden={hidden.includes(id) || undefined}
        data-open={(id.startsWith(GROUP_PREFIX) && expanded === id.slice(GROUP_PREFIX.length)) || undefined}
        className={`an-btn tb-order__item${id === 'done' ? ' an-btn--primary' : ''}${groupOf(id) ? ' an-btn--group' : ''}`}
        title={label}
        aria-label={label}
        onPointerDown={(e) => onPointerDown(e, id)}
        onContextMenu={(e) => contextMenu(e, id, inGroup)}
      >
        <Icon size={18} strokeWidth={1.5} absoluteStrokeWidth />
      </button>
    );
  };

  const newZoneText = !drop
    ? open
      ? t('settings.toolbarGroups.dropNew')
      : t('settings.toolbarGroups.idleHint')
    : drop.canNew
      ? t('settings.toolbarGroups.dropNew')
      : t('settings.toolbarGroups.cannotGroup');

  return (
    <div ref={rootRef} className="tb-order">
      <div ref={barRef} className="an-toolbar tb-order__bar">
        {LISTS.map((list, gi) => (
          <Fragment key={list}>
            {gi > 0 && <span className="an-sep" />}
            {order[list].map((id) => button(id, list))}
          </Fragment>
        ))}
      </div>
      <Button
        size="sm"
        variant="ghost"
        icon={RotateCcw}
        disabled={isDefault}
        onClick={() => {
          setExpanded(null);
          emit({ toolbarTools: [...TOOL_ORDER], toolbarActions: [...MID_ACTIONS], toolbarOrder: [...TAIL_ACTIONS], toolbarHidden: [], toolbarGroups: DEFAULT_GROUPS });
        }}
      >
        {t('settings.capture.toolbarOrderReset')}
      </Button>
      <div className="tb-order__zones">
        {open && (
          <div
            ref={groupBoxRef}
            className="tb-order__group"
            data-can-drop={(drop?.canJoin && !open.items.includes(dragging ?? '')) || undefined}
            data-over={drop?.over === 'group' || undefined}
          >
            <button type="button" className="tb-order__group-name" title={t('settings.toolbarGroups.rename')} onClick={() => void rename(open.id)}>
              {groupLabel(open, t)}
            </button>
            <div ref={groupRowRef} className="an-toolbar tb-order__bar tb-order__bar--group">
              {open.items.map((id) => button(id, { group: open.id }))}
              {!open.items.length && <span className="tb-order__empty">{t('settings.toolbarGroups.empty')}</span>}
            </div>
            <span className="tb-order__hint">{t('settings.toolbarGroups.expandedHint')}</span>
          </div>
        )}
        <div
          ref={newZoneRef}
          className="tb-order__new"
          data-can-drop={drop?.canNew || undefined}
          data-over={drop?.over === 'new' || undefined}
          data-disabled={(drop && !drop.canNew) || undefined}
        >
          <FolderPlus size={16} strokeWidth={1.5} />
          <span>{newZoneText}</span>
        </div>
      </div>
    </div>
  );
}
