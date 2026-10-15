// AI 服务的接口地址：用户常少填或多填一截，失焦时补齐成后端要的样子。
// - OpenAI 兼容：填到版本号为止（后端拼 /chat/completions、/models）。少了版本号补 /v1；
//   整个接口地址贴进来（…/v1/chat/completions）就去掉多出来的那截
// - Anthropic：填到域名为止（后端拼 /v1/messages），多填的 /v1、/v1/messages 去掉
// 两种都会补上协议头（本机地址补 http，其余补 https）、去掉末尾的斜杠。

export function normalizeAiBaseUrl(raw: string, kind: 'openai' | 'anthropic'): string {
  let url = raw.trim();
  if (!url) return '';
  if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(url)) {
    const local = /^(localhost|127\.|0\.0\.0\.0|\[::1\]|192\.168\.|10\.)/i.test(url);
    url = `${local ? 'http' : 'https'}://${url}`;
  }
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return raw.trim();
  }
  let path = parsed.pathname.replace(/\/+$/, '');
  if (kind === 'anthropic') {
    path = path.replace(/\/v1(\/messages|\/models)?$/i, '');
  } else {
    path = path.replace(/\/(chat\/completions|completions|models|responses|embeddings)$/i, '');
    // 路径里任何一段是版本号（v1、v4、v1beta）就算填到了；智谱是 /api/paas/v4
    if (!/(^|\/)v\d+[a-z]*(\/|$)/i.test(path)) path += '/v1';
  }
  return `${parsed.protocol}//${parsed.host}${path}${parsed.search}`;
}
