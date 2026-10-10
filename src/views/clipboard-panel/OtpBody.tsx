// 验证码（type = otp）的显示：大号验证码 + 下面一段短信原文。
// 设置里开了打码时验证码和原文里的那串数字都换成圆点，右边一只眼睛，鼠标放上去才显示。
// 剪贴板面板的卡片、主窗口的列表和详情都用它。

import clsx from 'clsx';
import { Eye } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useSettings } from '@/lib/settings';
import type { ClipItem } from '@/lib/types';

import './otp.css';

const DOT = '•';

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** 原文里的验证码换成圆点。短信里可能写成 123-456 / 123 456，中间的分隔也算进去 */
export function maskIn(text: string, code: string): string {
  if (!code) return text;
  const pattern = code
    .split('')
    .map(escapeRegExp)
    .join('[-\\s]?');
  return text.replace(new RegExp(pattern, 'gi'), (m) => m.replace(/[^-\s]/g, DOT));
}

/** 设置里开没开打码（默认开） */
export function useOtpMasked(): boolean {
  return useSettings()?.clipboard.otpMask ?? true;
}

export function OtpBody({ item, variant }: { item: ClipItem; variant: 'card' | 'row' | 'detail' | 'line' }) {
  const { t } = useTranslation();
  const masked = useOtpMasked();
  const [reveal, setReveal] = useState(false);
  const code = (item.preview ?? '').trim();
  const hidden = masked && !reveal;
  const shownCode = hidden ? DOT.repeat(code.length) : code;
  const sms = item.originText ? (hidden ? maskIn(item.originText, code) : item.originText) : null;
  const eye = masked && (
    <span
      className="otp__eye"
      title={t('clip.otpReveal')}
      aria-label={t('clip.otpReveal')}
      onMouseEnter={() => setReveal(true)}
      onMouseLeave={() => setReveal(false)}
    >
      <Eye size={variant === 'line' ? 13 : 15} strokeWidth={1.8} />
    </span>
  );

  if (variant === 'line') {
    return (
      <span className="otp otp--line">
        <span className="otp__code cn-mono">{shownCode}</span>
        {eye}
        {sms && <span className="otp__sms">{sms.replace(/\s+/g, ' ')}</span>}
      </span>
    );
  }
  return (
    <div className={clsx('otp', `otp--${variant}`)}>
      <div className="otp__head">
        <span className="otp__code cn-mono">{shownCode}</span>
        {eye}
      </div>
      {sms && <div className="otp__sms">{sms}</div>}
    </div>
  );
}
