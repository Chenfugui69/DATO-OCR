// 列出后端（Rust）里会让用户看到的中文文案，核对 src-tauri/src/i18n_en.rs 里是不是都有英文。
//
//   node scripts/backend-strings.mjs            # 列出还没翻译的
//   node scripts/backend-strings.mjs --all      # 列出全部
//
// 后端的提示条、报错都是直接写的中文；英文界面下由 i18n.rs 在送到界面之前按这张表换成英文
// （带 {…} 占位的句子按模板匹配）。日志（tracing::…!）和测试里的不算。

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', 'src-tauri', 'src');
const files = [];
const walk = (dir) => {
  for (const name of readdirSync(dir)) {
    if (name.startsWith('._')) continue;
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p);
    // 测试文件不算；settings.rs 里的中文是 AI 预设提示词的默认内容（存进设置的，不是界面文案）
    else if (name.endsWith('.rs') && !name.startsWith('i18n') && !name.endsWith('_tests.rs') && name !== 'settings.rs') files.push(p);
  }
};
walk(root);

const CJK = /[一-鿿]/;
/** 把 `{err}`、`{0}`、`{secs:.1}` 都归一成 `{}` */
export const normalize = (s) => s.replace(/\{\{/g, '\u0001').replace(/\}\}/g, '\u0002').replace(/\{[^{}]*\}/g, '{}').replace(/\u0001/g, '{{').replace(/\u0002/g, '}}');

const found = new Map();
for (const file of files) {
  let src = readFileSync(file, 'utf8');
  const cut = src.indexOf('#[cfg(test)]');
  if (cut >= 0) src = src.slice(0, cut);
  // 去掉注释和日志宏（日志宏可能跨行，按括号配对找结尾）
  src = src.replace(/^\s*\/\/.*$/gm, '');
  let out = '';
  for (let i = 0; i < src.length; ) {
    const m = /tracing::\w+!\(/y;
    m.lastIndex = i;
    if (m.test(src)) {
      let depth = 1;
      let j = m.lastIndex;
      let inStr = false;
      for (; j < src.length && depth > 0; j++) {
        const c = src[j];
        if (inStr) {
          if (c === '\\') j++;
          else if (c === '"') inStr = false;
        } else if (c === '"') inStr = true;
        else if (c === '(') depth++;
        else if (c === ')') depth--;
      }
      i = j;
    } else out += src[i++];
  }
  for (const m of out.matchAll(/"((?:[^"\\]|\\.)*)"/g)) {
    const text = m[1].replace(/\\n/g, '\n').replace(/\\"/g, '"').replace(/\\\n\s*/g, '');
    if (!CJK.test(text)) continue;
    const key = normalize(text);
    if (!found.has(key)) found.set(key, relative(root, file));
  }
}

const table = readFileSync(join(root, 'i18n_en.rs'), 'utf8');
const have = new Set([...table.matchAll(/^\s*\(\s*"((?:[^"\\]|\\.)*)"\s*,/gm)].map((m) => m[1].replace(/\\n/g, '\n').replace(/\\"/g, '"')));
const all = process.argv.includes('--all');
let missing = 0;
for (const [key, file] of found) {
  if (all || !have.has(key)) {
    if (!have.has(key)) missing++;
    console.log(`${file}\t${JSON.stringify(key)}`);
  }
}
console.error(`\n后端文案 ${found.size} 条，缺英文 ${missing} 条`);
