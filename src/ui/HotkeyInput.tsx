// 快捷键录入框：点一下进入录制，按下组合键即保存；Esc 取消、Backspace 清除。
// 录制期间暂停全部全局热键，否则按下的组合会直接触发功能。

import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { system } from '@/lib/ipc';

import { Kbd } from './controls';

const MODIFIER_KEYS = new Set(['Control', 'Shift', 'Alt', 'Meta', 'OS']);

/** KeyboardEvent → global-hotkey 语法（`Ctrl+Alt+T`、`F1`、`Super+Shift+S`）。 */
export function accelFromEvent(e: KeyboardEvent): string | null {
  if (MODIFIER_KEYS.has(e.key)) return null;
  const code = e.code;
  let key: string | null = null;
  if (/^Key[A-Z]$/.test(code)) key = code.slice(3);
  else if (/^Digit[0-9]$/.test(code)) key = code.slice(5);
  else if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) key = code;
  else {
    const map: Record<string, string> = {
      Space: 'Space',
      Enter: 'Enter',
      Tab: 'Tab',
      Backquote: '`',
      Minus: '-',
      Equal: '=',
      BracketLeft: '[',
      BracketRight: ']',
      Backslash: '\\',
      Semicolon: ';',
      Quote: "'",
      Comma: ',',
      Period: '.',
      Slash: '/',
      PrintScreen: 'PrintScreen',
      Insert: 'Insert',
      Delete: 'Delete',
      Home: 'Home',
      End: 'End',
      PageUp: 'PageUp',
      PageDown: 'PageDown',
      ArrowUp: 'Up',
      ArrowDown: 'Down',
      ArrowLeft: 'Left',
      ArrowRight: 'Right',
    };
    key = map[code] ?? null;
  }
  if (!key) return null;
  const mods: string[] = [];
  if (e.ctrlKey) mods.push('Ctrl');
  if (e.altKey) mods.push('Alt');
  if (e.shiftKey) mods.push('Shift');
  if (e.metaKey) mods.push('Super');
  // 不带修饰键的普通字母会让整个系统没法打这个字，只允许 F 键 / PrintScreen 单独使用
  if (mods.length === 0 && !/^F\d+$/.test(key) && key !== 'PrintScreen') return null;
  return [...mods, key].join('+');
}

export function HotkeyInput({
  value,
  onChange,
  error,
}: {
  value: string;
  onChange: (accel: string) => void;
  error?: boolean;
}) {
  const { t } = useTranslation();
  const [recording, setRecording] = useState(false);
  const ref = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!recording) return;
    void system.suspendHotkeys();
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === 'Escape') {
        setRecording(false);
        return;
      }
      if (e.key === 'Backspace' && !e.ctrlKey && !e.altKey && !e.shiftKey) {
        onChange('');
        setRecording(false);
        return;
      }
      const accel = accelFromEvent(e);
      if (accel) {
        onChange(accel);
        setRecording(false);
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => {
      window.removeEventListener('keydown', onKey, true);
      void system.resumeHotkeys();
    };
  }, [recording, onChange]);

  return (
    <button
      ref={ref}
      type="button"
      className="cn-hotkey"
      data-recording={recording}
      data-error={error || undefined}
      onClick={() => setRecording((r) => !r)}
      onBlur={() => setRecording(false)}
    >
      {recording ? t('hotkey.recording') : value ? <Kbd keys={value} /> : t('hotkey.none')}
    </button>
  );
}
