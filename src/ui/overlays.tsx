// 浮层类组件：Tooltip / 菜单 / 对话框 / 窗口内 toast。

import * as RadixContext from '@radix-ui/react-context-menu';
import * as RadixDialog from '@radix-ui/react-dialog';
import * as RadixDropdown from '@radix-ui/react-dropdown-menu';
import * as RadixTooltip from '@radix-ui/react-tooltip';
import clsx from 'clsx';
import { CheckCircle2, Info, XCircle, type LucideIcon } from 'lucide-react';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { create } from 'zustand';

import { shortcutLabel } from '@/lib/platform';

import { Button, Icon, TextField } from './controls';

// ───────────────────────── Tooltip ─────────────────────────

export function TooltipProvider({ children }: { children: ReactNode }) {
  return (
    <RadixTooltip.Provider delayDuration={400} skipDelayDuration={200}>
      {children}
    </RadixTooltip.Provider>
  );
}

export function Tooltip({
  content,
  shortcut,
  side = 'bottom',
  children,
}: {
  content: ReactNode;
  shortcut?: string;
  side?: 'top' | 'bottom' | 'left' | 'right';
  children: ReactNode;
}) {
  return (
    <RadixTooltip.Root>
      <RadixTooltip.Trigger asChild>{children}</RadixTooltip.Trigger>
      <RadixTooltip.Portal>
        <RadixTooltip.Content className="cn-tooltip" side={side} sideOffset={6} collisionPadding={8}>
          {content}
          {shortcut && <span className="cn-tooltip__kbd">{shortcutLabel(shortcut)}</span>}
        </RadixTooltip.Content>
      </RadixTooltip.Portal>
    </RadixTooltip.Root>
  );
}

// ───────────────────────── 菜单 ─────────────────────────

export interface MenuItemSpec {
  label?: string;
  icon?: LucideIcon;
  shortcut?: string;
  danger?: boolean;
  disabled?: boolean;
  checked?: boolean;
  onSelect?: () => void;
  separator?: boolean;
  submenu?: MenuItemSpec[];
}

type Primitive = typeof RadixContext | typeof RadixDropdown;

function renderItems(P: Primitive, items: MenuItemSpec[]): ReactNode {
  return items.map((item, i) => {
    if (item.separator) return <P.Separator key={i} className="cn-menu__sep" />;
    if (item.submenu) {
      return (
        <P.Sub key={i}>
          <P.SubTrigger className="cn-menu__item">
            {item.icon && <Icon icon={item.icon} size={14} />}
            {item.label}
            <span className="cn-menu__shortcut">›</span>
          </P.SubTrigger>
          <P.Portal>
            <P.SubContent className="cn-menu" sideOffset={4}>
              {renderItems(P, item.submenu)}
            </P.SubContent>
          </P.Portal>
        </P.Sub>
      );
    }
    return (
      <P.Item
        key={i}
        className={clsx('cn-menu__item', item.danger && 'cn-menu__item--danger')}
        disabled={item.disabled}
        onSelect={item.onSelect}
      >
        {item.checked !== undefined ? (
          <span className="cn-menu__check">{item.checked ? '✓' : ''}</span>
        ) : (
          item.icon && <Icon icon={item.icon} size={14} />
        )}
        {item.label}
        {item.shortcut && <span className="cn-menu__shortcut">{shortcutLabel(item.shortcut)}</span>}
      </P.Item>
    );
  });
}

export function ContextMenu({ items, children }: { items: MenuItemSpec[]; children: ReactNode }) {
  return (
    <RadixContext.Root>
      <RadixContext.Trigger asChild>{children}</RadixContext.Trigger>
      <RadixContext.Portal>
        <RadixContext.Content className="cn-menu" collisionPadding={8}>
          {renderItems(RadixContext, items)}
        </RadixContext.Content>
      </RadixContext.Portal>
    </RadixContext.Root>
  );
}

export function DropdownMenu({
  items,
  children,
  align = 'end',
}: {
  items: MenuItemSpec[];
  children: ReactNode;
  align?: 'start' | 'center' | 'end';
}) {
  return (
    <RadixDropdown.Root>
      <RadixDropdown.Trigger asChild>{children}</RadixDropdown.Trigger>
      <RadixDropdown.Portal>
        <RadixDropdown.Content className="cn-menu" align={align} sideOffset={4} collisionPadding={8}>
          {renderItems(RadixDropdown, items)}
        </RadixDropdown.Content>
      </RadixDropdown.Portal>
    </RadixDropdown.Root>
  );
}

// ───────────────────────── 对话框 ─────────────────────────

interface DialogRequest {
  kind: 'confirm' | 'prompt';
  title: string;
  body?: string;
  confirmLabel?: string;
  danger?: boolean;
  initial?: string;
  placeholder?: string;
  resolve: (value: string | boolean | null) => void;
}

const useDialogStore = create<{ request: DialogRequest | null }>(() => ({ request: null }));

export function confirmDialog(opts: { title: string; body?: string; confirmLabel?: string; danger?: boolean }): Promise<boolean> {
  return new Promise((resolve) => {
    useDialogStore.setState({
      request: { kind: 'confirm', ...opts, resolve: (v) => resolve(v === true) },
    });
  });
}

export function promptDialog(opts: { title: string; initial?: string; placeholder?: string; confirmLabel?: string }): Promise<string | null> {
  return new Promise((resolve) => {
    useDialogStore.setState({
      request: { kind: 'prompt', ...opts, resolve: (v) => resolve(typeof v === 'string' ? v : null) },
    });
  });
}

export function DialogHost() {
  const { t } = useTranslation();
  const request = useDialogStore((s) => s.request);
  const [text, setText] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (request?.kind === 'prompt') setText(request.initial ?? '');
  }, [request]);

  const close = (value: string | boolean | null) => {
    request?.resolve(value);
    useDialogStore.setState({ request: null });
  };

  return (
    <RadixDialog.Root open={!!request} onOpenChange={(open) => !open && close(null)}>
      <RadixDialog.Portal>
        <RadixDialog.Overlay className="cn-dialog-overlay" />
        <RadixDialog.Content
          className="cn-dialog"
          onOpenAutoFocus={(e) => {
            if (request?.kind === 'prompt') {
              e.preventDefault();
              setTimeout(() => inputRef.current?.select(), 0);
            }
          }}
        >
          <RadixDialog.Title className="cn-dialog__title">{request?.title}</RadixDialog.Title>
          {request?.body && <RadixDialog.Description className="cn-dialog__body">{request.body}</RadixDialog.Description>}
          {request?.kind === 'prompt' && (
            <form
              onSubmit={(e) => {
                e.preventDefault();
                close(text);
              }}
              style={{ marginBottom: 'var(--cn-space-5)' }}
            >
              <TextField ref={inputRef} value={text} placeholder={request.placeholder} onChange={(e) => setText(e.target.value)} />
            </form>
          )}
          <div className="cn-dialog__actions">
            <Button onClick={() => close(null)}>{t('common.cancel')}</Button>
            <Button
              variant={request?.danger ? 'danger' : 'primary'}
              onClick={() => close(request?.kind === 'prompt' ? text : true)}
            >
              {request?.confirmLabel ?? t('common.ok')}
            </Button>
          </div>
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}

// ───────────────────────── 窗口内 toast ─────────────────────────

interface ToastItem {
  id: number;
  kind: 'success' | 'error' | 'info';
  message: string;
  leaving?: boolean;
}

const useToastStore = create<{ toasts: ToastItem[] }>(() => ({ toasts: [] }));
let toastSeq = 0;

function push(kind: ToastItem['kind'], message: string) {
  const id = ++toastSeq;
  useToastStore.setState((s) => ({ toasts: [...s.toasts.slice(-2), { id, kind, message }] }));
  const stay = kind === 'error' ? 3000 : 1500;
  setTimeout(() => {
    useToastStore.setState((s) => ({ toasts: s.toasts.map((t) => (t.id === id ? { ...t, leaving: true } : t)) }));
    setTimeout(() => useToastStore.setState((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })), 170);
  }, stay);
}

export const notify = {
  success: (m: string) => push('success', m),
  error: (m: unknown) => push('error', m instanceof Error ? m.message : String(m)),
  info: (m: string) => push('info', m),
};

const toastIcons = { success: CheckCircle2, error: XCircle, info: Info };

export function Toaster() {
  const toasts = useToastStore((s) => s.toasts);
  return (
    <div className="cn-toaster" aria-live="polite">
      {toasts.map((t) => (
        <div key={t.id} className={clsx('cn-toast cn-glass', `cn-toast--${t.kind}`)} data-leaving={t.leaving || undefined}>
          <Icon icon={toastIcons[t.kind]} size={16} />
          <span>{t.message}</span>
        </div>
      ))}
    </div>
  );
}
