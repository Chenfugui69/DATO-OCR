// 朗读按钮：喇叭；正在读这一处时变成方块（停止），再点一下停。按钮没了（卡片收起、窗口关掉）也停。

import { Square, Volume2 } from 'lucide-react';
import { useEffect, useId } from 'react';
import { useTranslation } from 'react-i18next';

import { stopSpeaking, toggleSpeak, useSpeech } from '@/lib/speech';
import { IconButton } from '@/ui/controls';
import { notify } from '@/ui/overlays';

export function SpeakButton({ text, lang = 'auto', label, className }: { text: string; lang?: string; label?: string; className?: string }) {
  const { t } = useTranslation();
  const key = useId();
  const active = useSpeech((s) => s.key === key);
  useEffect(
    () => () => {
      if (useSpeech.getState().key === key) stopSpeaking();
    },
    [key],
  );
  return (
    <IconButton
      icon={active ? Square : Volume2}
      size="sm"
      className={className}
      active={active}
      label={active ? t('translate.stopSpeak') : (label ?? t('translate.speak'))}
      disabled={!text.trim()}
      onClick={() => void toggleSpeak(key, text, lang).catch(notify.error)}
    />
  );
}
