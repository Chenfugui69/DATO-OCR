// 所有界面文案走 i18n（规格 00 §6.6）。第一版只出简体中文，英文同步写好。

import { invoke } from '@tauri-apps/api/core';
import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';

import { isMac } from '@/lib/platform';

import enUS from './en-US';
import { macEnUS, macZhCN } from './mac';
import zhCN from './zh-CN';

type Tree = { [key: string]: string | Tree };

/** 把 macOS 的覆盖文案叠到基础文案上（只换叶子，结构不变）。 */
function overlay<T>(base: T, patch: Tree): T {
  const out = { ...base } as Tree;
  for (const [key, value] of Object.entries(patch)) {
    const current = out[key];
    out[key] = typeof value === 'string' || typeof current !== 'object' ? value : overlay(current, value);
  }
  return out as T;
}

void i18n.use(initReactI18next).init({
  resources: {
    'zh-CN': { translation: isMac ? overlay(zhCN, macZhCN) : zhCN },
    'en-US': { translation: isMac ? overlay(enUS, macEnUS) : enUS },
  },
  lng: 'zh-CN',
  fallbackLng: 'zh-CN',
  interpolation: { escapeValue: false },
  returnNull: false,
});

function apply(lang: string): void {
  if (i18n.language !== lang && (lang === 'zh-CN' || lang === 'en-US')) {
    void i18n.changeLanguage(lang);
    document.documentElement.lang = lang;
  }
}

/** 按设置切界面语言。「跟随系统」（system）时问后端系统是什么语言：托盘菜单、提示条也是按它定的，两边要一致。 */
export function setLanguage(lang: string): void {
  if (lang !== 'system') return apply(lang);
  invoke<string>('ui_language').then(apply, () => apply(navigator.language.toLowerCase().startsWith('zh') ? 'zh-CN' : 'en-US'));
}

export default i18n;
