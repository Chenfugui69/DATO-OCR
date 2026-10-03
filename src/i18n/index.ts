// 所有界面文案走 i18n（规格 00 §6.6）。第一版只出简体中文，英文同步写好。

import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';

import enUS from './en-US';
import zhCN from './zh-CN';

void i18n.use(initReactI18next).init({
  resources: {
    'zh-CN': { translation: zhCN },
    'en-US': { translation: enUS },
  },
  lng: 'zh-CN',
  fallbackLng: 'zh-CN',
  interpolation: { escapeValue: false },
  returnNull: false,
});

export function setLanguage(lang: string): void {
  if (i18n.language !== lang && (lang === 'zh-CN' || lang === 'en-US')) {
    void i18n.changeLanguage(lang);
    document.documentElement.lang = lang;
  }
}

export default i18n;
