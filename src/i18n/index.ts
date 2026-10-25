// 所有界面文案走 i18n（规格 00 §6.6）。简体中文是原文，英文、繁体中文、日文、韩文是译文（类型都是 Dict，
// 少了键 tsc 报错）。漏翻的键：繁体退回简体，其余退回英文。

import { invoke } from '@tauri-apps/api/core';
import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';

import { isMac } from '@/lib/platform';

import enUS from './en-US';
import jaJP from './ja-JP';
import koKR from './ko-KR';
import { macEnUS, macZhCN } from './mac';
import zhCN from './zh-CN';
import zhTW from './zh-TW';

/** 界面能选的语言（设置里的下拉、跟随系统时的结果都在这几种里） */
export const UI_LANGUAGES = ['zh-CN', 'zh-TW', 'en-US', 'ja-JP', 'ko-KR'] as const;
/** 语言选单里显示的名字：用各自的语言写，不跟着界面语言变（看不懂当前界面的人也找得到自己的语言） */
export const UI_LANGUAGE_NAMES: Record<(typeof UI_LANGUAGES)[number], string> = {
  'zh-CN': '简体中文',
  'zh-TW': '繁體中文',
  'en-US': 'English',
  'ja-JP': '日本語',
  'ko-KR': '한국어',
};

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
    'zh-TW': { translation: zhTW },
    'ja-JP': { translation: jaJP },
    'ko-KR': { translation: koKR },
  },
  lng: 'zh-CN',
  fallbackLng: { 'zh-TW': ['zh-CN'], 'ja-JP': ['en-US'], 'ko-KR': ['en-US'], default: ['zh-CN'] },
  interpolation: { escapeValue: false },
  returnNull: false,
});

function apply(lang: string): void {
  if (i18n.language !== lang && (UI_LANGUAGES as readonly string[]).includes(lang)) {
    void i18n.changeLanguage(lang);
    // 字体按语言换（简繁日韩的汉字字形不一样，见 tokens.css 的 :lang()）
    document.documentElement.lang = lang;
  }
}

/** 浏览器给的语言（zh-TW、ja……）→ 界面语言。问不到后端时用 */
function fromNavigator(): string {
  const l = navigator.language.toLowerCase();
  if (l.startsWith('zh')) return /hant|-tw|-hk|-mo/.test(l) ? 'zh-TW' : 'zh-CN';
  if (l.startsWith('ja')) return 'ja-JP';
  if (l.startsWith('ko')) return 'ko-KR';
  return 'en-US';
}

/** 按设置切界面语言。「跟随系统」（system）时问后端系统是什么语言：托盘菜单、提示条也是按它定的，两边要一致。 */
export function setLanguage(lang: string): void {
  if (lang !== 'system') return apply(lang);
  invoke<string>('ui_language').then(apply, () => apply(fromNavigator()));
}

/** 现在是不是中文界面（简体或繁体）。服务商名字、更新说明这些只有中英两份的，非中文界面用英文那份 */
export const isChineseUi = () => i18n.language.startsWith('zh');

export default i18n;
