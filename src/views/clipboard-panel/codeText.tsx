// 剪贴板卡片里的代码：认出是代码后用等宽字、按关键字 / 字符串 / 数字 / 注释上色（参照 Paste 对 Xcode 的显示）。
// 只是卡片上的预览（最多几十行），不追求准确的语法分析：一个正则从头扫到尾，够看出"这是代码"就行。

/** 编辑器、终端：从这些应用复制的多半是代码 */
const CODE_APPS = /^(xcode|visual studio code|code|cursor|zed|nova|sublime text|bbedit|coteditor|intellij idea|pycharm|webstorm|goland|clion|rustrover|android studio|android studio preview|terminal|iterm2?|warp|ghostty|windows terminal|windowsterminal|powershell|cmd|neovide|vim|macvim|hbuilder ?x?|微信开发者工具)$/i;

/** 不看来源时靠这些特征判断：每样占一分，够分才算代码（免得把普通段落当成代码） */
const CODE_HINTS: RegExp[] = [
  /^\s*(import|from|package|using|#include|#import)\b/m,
  /\b(function|const|let|var|def|class|struct|enum|interface|impl|fn|func|public|private|static|return|async|await)\b/,
  /[{};]\s*$/m,
  /=>|->|::|===|!==|&&|\|\|/,
  /^\s{2,}\S/m,
  /\b\w+\([^()]*\)\s*[{:;]?\s*$/m,
  /^\s*(\/\/|#\s|\/\*|<!--|--\s)/m,
];

export function looksLikeCode(text: string, sourceApp: string | null): boolean {
  const lines = text.split('\n');
  if (lines.length < 2) return false;
  if (sourceApp && CODE_APPS.test(sourceApp.trim())) return true;
  // 中文句子多的是普通文字
  const cjk = (text.match(/[一-鿿]/g) ?? []).length;
  if (cjk > text.length * 0.2) return false;
  return CODE_HINTS.filter((r) => r.test(text)).length >= 3;
}

const KEYWORDS = new Set(
  (
    'abstract and as async await break case catch class const continue def default defer del do elif else enum export extends ' +
    'extension false final finally fn for from func function go guard if impl import in init interface is let loop match mod ' +
    'module mut new nil none not null or package pass private protected protocol pub public raise return self Self static struct ' +
    'super switch this throw throws trait true try type typeof undefined use var void where while with yield'
  ).split(' '),
);

/** 注释 | 字符串 | 数字 | 单词 */
const TOKEN = /(\/\/[^\n]*|\/\*[\s\S]*?(?:\*\/|$)|#(?![![{])[^\n]*|<!--[\s\S]*?(?:-->|$))|("(?:\\.|[^"\\\n])*"?|'(?:\\.|[^'\\\n])*'?|`(?:\\.|[^`\\])*`?)|(\b\d[\d_]*(?:\.\d+)?(?:e[+-]?\d+)?\b|\b0x[\da-f]+\b)|([A-Za-z_$][\w$]*)/gi;

export function highlightCode(text: string): React.ReactNode[] {
  const out: React.ReactNode[] = [];
  let last = 0;
  let key = 0;
  for (const m of text.matchAll(TOKEN)) {
    const at = m.index ?? 0;
    if (at > last) out.push(text.slice(last, at));
    const [whole, comment, str, num, word] = m;
    let cls: string | null = null;
    if (comment) cls = 'tok-com';
    else if (str) cls = 'tok-str';
    else if (num) cls = 'tok-num';
    else if (word) {
      if (KEYWORDS.has(word)) cls = 'tok-kw';
      else if (/^[A-Z][a-z]/.test(word)) cls = 'tok-type';
      else if (text[at + word.length] === '(') cls = 'tok-fn';
    }
    out.push(cls ? (
      <span key={key++} className={cls}>
        {whole}
      </span>
    ) : (
      whole
    ));
    last = at + whole.length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}
