// AI 设置顶上的新手教程：没用过 AI 接口的人不知道"模型""token""密钥"是什么，也不知道去哪弄。
// 这里只讲怎么做（去智谱开放平台注册、拿密钥、回来填上），每一步带能直接点开的网页链接，操作让用户自己来。
// 还没添加任何 AI 服务时默认展开，加过之后收成一行，想看再点开。

import { BookOpen, ChevronDown, ExternalLink } from 'lucide-react';
import { Fragment, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { system } from '@/lib/ipc';
import { notify } from '@/ui/overlays';

/** 注册 / 登录，登录完直接到 API Keys 页面 */
const REGISTER_URL = 'https://bigmodel.cn/login?redirect=%2Fapikey%2Fplatform';
const API_KEYS_URL = 'https://bigmodel.cn/apikey/platform';
const PRICING_URL = 'https://bigmodel.cn/pricing';
const DOCS_URL = 'https://docs.bigmodel.cn/cn/guide/start/quick-start';

/** 文案里的 [名字] 换成对应的元素（模型名、接口地址做成能选中复制的小代码块） */
const fill = (text: string, parts: Record<string, ReactNode>) =>
  text.split(/\[(\w+)\]/).map((seg, i) => (i % 2 ? <Fragment key={i}>{parts[seg] ?? seg}</Fragment> : seg));

function Link({ href, children }: { href: string; children?: ReactNode }) {
  return (
    <a
      className="ai-guide__link"
      href={href}
      onClick={(e) => {
        e.preventDefault();
        void system.openUrl(href).catch(notify.error);
      }}
    >
      {children}
      <ExternalLink size={11} strokeWidth={2} />
    </a>
  );
}

export function AiGuide({ hasProvider }: { hasProvider: boolean }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(!hasProvider);
  const code = (s: string) => <code className="ai-guide__code cn-selectable">{s}</code>;
  return (
    <div className="ai-guide" data-open={open || undefined}>
      <button type="button" className="ai-guide__head" aria-expanded={open} onClick={() => setOpen((v) => !v)}>
        <BookOpen size={15} strokeWidth={1.75} />
        <span className="ai-guide__title">{t('settings.ai.guide.title')}</span>
        <span className="ai-guide__sub">{t('settings.ai.guide.sub')}</span>
        <ChevronDown size={14} className="ai-guide__chev" />
      </button>
      {open && (
        <div className="ai-guide__body">
          <p className="ai-guide__intro">{t('settings.ai.guide.intro')}</p>
          <ol className="ai-guide__steps">
            <li>
              <b>{t('settings.ai.guide.s1')}</b>
              <span>
                {t('settings.ai.guide.s1d')} <Link href={REGISTER_URL}>{t('settings.ai.guide.s1link')}</Link>
              </span>
            </li>
            <li>
              <b>{t('settings.ai.guide.s2')}</b>
              <span>
                {t('settings.ai.guide.s2d')} <Link href={API_KEYS_URL}>{t('settings.ai.guide.s2link')}</Link>
              </span>
            </li>
            <li>
              <b>{t('settings.ai.guide.s3')}</b>
              <span>{fill(t('settings.ai.guide.s3d'), { url: code('https://open.bigmodel.cn/api/paas/v4') })}</span>
            </li>
            <li>
              <b>{t('settings.ai.guide.s4')}</b>
              <span>{fill(t('settings.ai.guide.s4d'), { text: code('glm-4.7-flash'), vision: code('glm-4.6v-flash') })}</span>
            </li>
            <li>
              <b>{t('settings.ai.guide.s5')}</b>
              <span>{fill(t('settings.ai.guide.s5d'), { text: code('glm-4.7-flash'), vision: code('glm-4.6v-flash') })}</span>
            </li>
          </ol>
          <p className="ai-guide__more">
            {t('settings.ai.guide.more')} <Link href={PRICING_URL}>{t('settings.ai.guide.pricing')}</Link>
            <Link href={DOCS_URL}>{t('settings.ai.guide.docs')}</Link>
          </p>
        </div>
      )}
    </div>
  );
}
