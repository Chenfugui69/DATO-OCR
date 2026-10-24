// 朗读（翻译结果、输入翻译的原文旁边的小喇叭）。
// macOS 用系统的语音合成（后端，语速 / 音调 / 声音按设置），别的平台后端报 "unsupported"，改用浏览器自带的
// speechSynthesis（同样按设置）。正在读的那一处按钮变成"停止"，再点一下停；面板收起时也停。

import { create } from 'zustand';

import { system } from '@/lib/ipc';
import { useSettingsStore } from '@/lib/settings';

/** 我们的语言代码 → 浏览器语音合成认的语言标签 */
const TAGS: Record<string, string> = {
  zh: 'zh-CN',
  'zh-TW': 'zh-TW',
  en: 'en-US',
  ja: 'ja-JP',
  ko: 'ko-KR',
  fr: 'fr-FR',
  de: 'de-DE',
  es: 'es-ES',
  ru: 'ru-RU',
  pt: 'pt-BR',
  it: 'it-IT',
  ar: 'ar-SA',
  th: 'th-TH',
  vi: 'vi-VN',
};

/** 正在读的是哪一处（按钮自己起的名字）；null = 没在读 */
export const useSpeech = create<{ key: string | null }>(() => ({ key: null }));

/** 这次是不是用的浏览器语音（判断"读完了没有"要问不同的地方） */
let viaWeb = false;
let poll: number | undefined;

/** 读出一段文字；`lang` 不知道就传 auto（按文字猜） */
export async function speak(text: string, lang = 'auto'): Promise<void> {
  if (!text.trim()) return;
  try {
    await system.speak(text, lang);
    viaWeb = false;
  } catch (err) {
    if (typeof window.speechSynthesis === 'undefined') throw err;
    viaWeb = true;
    const sp = useSettingsStore.getState().settings?.translate.speech;
    window.speechSynthesis.cancel();
    const u = new SpeechSynthesisUtterance(text);
    const tag = TAGS[lang];
    if (tag) u.lang = tag;
    const base = (tag ?? '').split('-')[0] ?? '';
    const wanted = sp?.voices[lang] ?? sp?.voices[base];
    const voice = wanted ? window.speechSynthesis.getVoices().find((v) => v.voiceURI === wanted) : undefined;
    if (voice) u.voice = voice;
    u.rate = sp?.rate ?? 1;
    u.pitch = sp?.pitch ?? 1;
    window.speechSynthesis.speak(u);
  }
}

export function stopSpeaking() {
  window.clearInterval(poll);
  useSpeech.setState({ key: null });
  void system.speakStop().catch(() => undefined);
  if (typeof window.speechSynthesis !== 'undefined') window.speechSynthesis.cancel();
}

async function isSpeaking(): Promise<boolean> {
  if (viaWeb) return typeof window.speechSynthesis !== 'undefined' && window.speechSynthesis.speaking;
  return system.speakStatus().catch(() => false);
}

/** 喇叭按钮点一下：没在读就读，正在读这一处就停 */
export async function toggleSpeak(key: string, text: string, lang = 'auto'): Promise<void> {
  if (useSpeech.getState().key === key) {
    stopSpeaking();
    return;
  }
  await speak(text, lang);
  useSpeech.setState({ key });
  const started = performance.now();
  window.clearInterval(poll);
  // 读完了按钮变回喇叭。刚开始那一下系统可能还没进入"在读"，先别当成读完
  poll = window.setInterval(() => {
    void isSpeaking().then((on) => {
      if (on || performance.now() - started < 600) return;
      window.clearInterval(poll);
      if (useSpeech.getState().key === key) useSpeech.setState({ key: null });
    });
  }, 300);
}
