// 检查代码里用到的 t('a.b.c') 在中英文文案里都存在，且两种语言的 key 一致。
//
//   node scripts/check-i18n.mjs
//
// 只认字面量 key；模板字符串（t(`settings.translate.${id}`)）按前缀检查该分组存在。

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', 'src');
const load = async (name) => (await import(`file:///${join(root, 'i18n', name).replace(/\\/g, '/')}`)).default;
const zh = await load('zh-CN.ts');
const en = await load('en-US.ts');

const flatten = (obj, prefix = '', out = new Set()) => {
  for (const [k, v] of Object.entries(obj)) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (v && typeof v === 'object') flatten(v, key, out);
    else out.add(key);
  }
  return out;
};
const zhKeys = flatten(zh);
const enKeys = flatten(en);

const files = [];
const walk = (dir) => {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p);
    else if (/\.(tsx?|mts)$/.test(name) && !p.includes(`${join('src', 'i18n')}`)) files.push(p);
  }
};
walk(root);

const problems = [];
for (const file of files) {
  const src = readFileSync(file, 'utf8');
  for (const m of src.matchAll(/\bt\(\s*'([^']+)'/g)) {
    const key = m[1];
    // i18next 复数：key_one / key_other
    const has = (set) => set.has(key) || set.has(`${key}_one`) || set.has(`${key}_other`);
    if (!has(zhKeys)) problems.push(`zh 缺 ${key}  (${file.slice(root.length + 1)})`);
    if (!has(enKeys)) problems.push(`en 缺 ${key}  (${file.slice(root.length + 1)})`);
  }
  for (const m of src.matchAll(/\bt\(\s*`([^`$]+)\$\{/g)) {
    const prefix = m[1];
    if (![...zhKeys].some((k) => k.startsWith(prefix))) problems.push(`zh 缺分组 ${prefix}*  (${file.slice(root.length + 1)})`);
  }
}
for (const k of zhKeys) if (!enKeys.has(k)) problems.push(`en 少了 zh 里有的 ${k}`);
for (const k of enKeys) if (!zhKeys.has(k)) problems.push(`zh 少了 en 里有的 ${k}`);

if (problems.length) {
  console.log([...new Set(problems)].join('\n'));
  process.exit(1);
}
console.log(`i18n OK：${files.length} 个文件，${zhKeys.size} 个 key`);
