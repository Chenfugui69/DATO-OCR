// 本地模拟 AI 服务，测 AI 对话用（不花钱、不外发数据）。
//
//   node scripts/test/mock-ai.mjs [端口，默认 8765]
//
// OpenAI 兼容：GET /v1/models、POST /v1/chat/completions（stream）
// Anthropic：  GET /v1/models（带 anthropic-version 头时）、POST /v1/messages（stream）
// 回答内容 = 一段固定的 Markdown + 收到的最后一条提问的摘要（字数、带了几张图）。
// 每次请求的摘要写到同目录 mock-ai.log，方便核对上下文和图片有没有带上。

import { appendFileSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const port = Number(process.argv[2] ?? 8765);
const logFile = join(dirname(fileURLToPath(import.meta.url)), 'mock-ai.log');
writeFileSync(logFile, '');
const log = (line) => appendFileSync(logFile, `${new Date().toISOString()} ${line}\n`);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function summarize(messages) {
  const last = messages[messages.length - 1] ?? {};
  const parts = Array.isArray(last.content) ? last.content : [{ type: 'text', text: last.content ?? '' }];
  const text = parts.filter((p) => p.type === 'text').map((p) => p.text).join('');
  const images = parts.filter((p) => p.type === 'image_url' || p.type === 'image').length;
  return { text, images, turns: messages.length };
}

function answer(sum) {
  return [
    '**模拟回答**：收到了 ',
    `${sum.text.length} 个字的提问`,
    sum.images ? `，还有 ${sum.images} 张图` : '',
    `（第 ${Math.ceil(sum.turns / 2)} 轮）。\n\n`,
    '- 列表第一项\n- 列表第二项\n\n',
    '```js\nconsole.log("hello");\n```\n',
  ];
}

const server = createServer(async (req, res) => {
  let body = '';
  for await (const chunk of req) body += chunk;
  const anthropic = !!req.headers['anthropic-version'];
  log(`${req.method} ${req.url} anthropic=${anthropic} auth=${req.headers.authorization ? 'bearer' : req.headers['x-api-key'] ? 'x-api-key' : 'none'}`);

  if (req.method === 'GET' && req.url.startsWith('/v1/models')) {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ data: [{ id: 'mock-fast' }, { id: 'mock-vision' }, { id: 'mock-large' }] }));
    return;
  }
  const json = body ? JSON.parse(body) : {};
  if (req.method === 'POST' && (req.url === '/v1/chat/completions' || req.url === '/v1/messages')) {
    const messages = json.messages ?? [];
    const sum = summarize(messages);
    log(`chat model=${json.model} system=${JSON.stringify((json.system ?? messages.find((m) => m.role === 'system')?.content ?? '').slice(0, 30))} last=${JSON.stringify(sum.text.slice(0, 80))} images=${sum.images} turns=${sum.turns}`);
    if (json.model === 'mock-error') {
      res.writeHead(401, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ error: { message: 'Invalid API key' } }));
      return;
    }
    res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });
    for (const piece of answer(sum)) {
      // 故意把多字节字符拆开发，考验客户端按字节攒行
      for (const ch of piece.match(/[\s\S]{1,3}/g) ?? []) {
        const data = req.url === '/v1/messages'
          ? { type: 'content_block_delta', delta: { type: 'text_delta', text: ch } }
          : { choices: [{ delta: { content: ch } }] };
        const line = Buffer.from(`data: ${JSON.stringify(data)}\n\n`);
        const cut = Math.floor(line.length / 2);
        res.write(line.subarray(0, cut));
        await sleep(5);
        res.write(line.subarray(cut));
        await sleep(25);
      }
    }
    res.end(req.url === '/v1/messages' ? `data: ${JSON.stringify({ type: 'message_stop' })}\n\n` : 'data: [DONE]\n\n');
    return;
  }
  res.writeHead(404);
  res.end();
});
server.listen(port, '127.0.0.1', () => console.log(`mock AI on http://127.0.0.1:${port}`));
