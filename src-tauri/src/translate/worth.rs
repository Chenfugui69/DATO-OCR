//! 选中的这段字值不值得弹划词按钮。网址、路径、文件名、一串数字或编号、代码用不着翻译，按钮冒出来只是打扰。
//!
//! 只在系统肯告诉我们选中了什么字时才判断（见 `SelectionEvent::Selected::text`）。拿不准的一律算"值得"：
//! 多弹一次只是多一颗小圆点，少弹一次是想翻译时找不到入口。快捷键、按住修饰键选字不走这里，什么都能翻。

/// 这段字像是给人读的（要弹按钮）。
pub fn worth_translating(text: &str) -> bool {
    let t = text.trim();
    // 一个字母、文字都没有：数字、符号、表情
    if !t.chars().any(char::is_alphabetic) {
        return false;
    }
    if is_path(t) {
        return false;
    }
    // 每一截都是网址、文件名、编号这类东西
    if t.split_whitespace()
        .all(|token| is_link(token) || is_machine_token(token))
    {
        return false;
    }
    !looks_like_code(t)
}

/// 网址、邮箱。
fn is_link(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    if lower.starts_with("www.") || lower.starts_with("mailto:") {
        return true;
    }
    // 协议://……
    if let Some((scheme, rest)) = lower.split_once("://") {
        return !rest.is_empty()
            && !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
    }
    false
}

/// 一整行是个文件路径（中间可以有空格）：/Users/…、~/…、./…、C:\…
fn is_path(text: &str) -> bool {
    if text.contains('\n') {
        return false;
    }
    let rest = ["~/", "./", "../", "/"]
        .iter()
        .find_map(|p| text.strip_prefix(p))
        .or_else(|| {
            let b = text.as_bytes();
            (b.len() > 3
                && b[0].is_ascii_alphabetic()
                && b[1] == b':'
                && (b[2] == b'\\' || b[2] == b'/'))
                .then(|| &text[3..])
        });
    // "/ 后面是一句话" 不算；路径里总得再有一级，或者带扩展名
    rest.is_some_and(|rest| {
        !rest.starts_with(char::is_whitespace)
            && !rest.starts_with('/')
            && (rest.contains(['/', '\\']) || rest.contains('.') || !rest.contains(' '))
    })
}

/// 单独一截、不是普通单词的英文数字串：文件名、域名、版本号、哈希、变量名、带括号的调用……
/// 普通单词 = 字母开头，只有字母、数字、连字符、撇号（"state-of-the-art"、"don't"、"COVID-19"）。
fn is_machine_token(token: &str) -> bool {
    if !token.is_ascii() {
        return false;
    }
    // 句子里带出来的标点、引号、括号不算
    let core = token.trim_matches(|c: char| ".,;:!?\"'()[]<>".contains(c));
    if core.is_empty() {
        return true;
    }
    let word = core.starts_with(|c: char| c.is_ascii_alphabetic())
        && core
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '\'');
    // 一长串十六进制（哈希、编号）
    let hex = core.len() >= 12
        && core.chars().any(|c| c.is_ascii_digit())
        && core.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    !word || hex
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF)
}

/// 行首是这些词（后面跟空格或括号）多半是代码或命令
const KEYWORDS: &[&str] = &[
    "import",
    "package",
    "using",
    "function",
    "const",
    "let",
    "var",
    "def",
    "class",
    "struct",
    "enum",
    "interface",
    "impl",
    "fn",
    "func",
    "pub",
    "public",
    "private",
    "protected",
    "static",
    "return",
    "async",
    "await",
    "export",
    "namespace",
    "typedef",
    "void",
    "elif",
    "SELECT",
    "INSERT",
    "UPDATE",
    "CREATE",
    // 命令行
    "sudo",
    "git",
    "npm",
    "pnpm",
    "yarn",
    "npx",
    "cargo",
    "pip",
    "pip3",
    "brew",
    "docker",
    "kubectl",
    "curl",
    "wget",
    "apt",
    "apt-get",
    "ssh",
    "chmod",
    "mkdir",
    "cd",
    "ls",
    "rm",
    "cp",
    "mv",
    "cat",
    "grep",
    "echo",
    "make",
    "node",
    "python",
    "python3",
];
/// 行首原样是这些
const PREFIXES: &[&str] = &[
    "#include", "#import", "#define", "#!", "if (", "for (", "while (", "switch (", "} else", "$ ",
    "</",
];

fn starts_like_code(line: &str) -> bool {
    let letter_after = |p: char| {
        line.strip_prefix(p)
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_alphabetic()))
    };
    PREFIXES.iter().any(|p| line.starts_with(p))
        // 装饰器 @name、标签 <div
        || letter_after('@')
        || letter_after('<')
        || KEYWORDS
            .iter()
            .any(|k| line.strip_prefix(k).is_some_and(|rest| rest.starts_with([' ', '('])))
}

/// 这一行有几样代码的特征。
fn code_signals(line: &str) -> usize {
    let t = line.trim();
    let bytes = t.as_bytes();
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut n = 0;
    // 以 ; { } 结尾，或者整行只有括号标点
    if t.ends_with([';', '{', '}']) || t.ends_with("):") || !t.chars().any(char::is_alphanumeric) {
        n += 1;
    }
    if [
        "=>", "->", "::", "==", "!=", "&&", "||", "+=", "-=", ":=", "</", "/>",
    ]
    .iter()
    .any(|op| t.contains(op))
    {
        n += 1;
    }
    if starts_like_code(t) {
        n += 1;
    }
    // 名字紧跟着括号：foo(…)
    if bytes.windows(2).any(|w| ident(w[0]) && w[1] == b'(') && t.contains(')') {
        n += 1;
    }
    // 下划线连起来的名字、命令行的参数（-x、--flag）、$变量
    let snake = bytes
        .windows(3)
        .any(|w| w[1] == b'_' && ident(w[0]) && ident(w[2]));
    let flag = t.split_whitespace().skip(1).any(|w| {
        let name = w.trim_start_matches('-');
        (1..=2).contains(&(w.len() - name.len()))
            && name.starts_with(|c: char| c.is_ascii_alphabetic())
    });
    let var = bytes
        .windows(2)
        .any(|w| w[0] == b'$' && (ident(w[1]) || w[1] == b'{'));
    if snake || flag || var {
        n += 1;
    }
    // 赋值、键值
    if t.contains(" = ") || t.contains("\": ") || t.contains("=\"") {
        n += 1;
    }
    n
}

/// 这一行读着像一句话：普通单词够多、占了大半。
fn reads_like_prose(line: &str) -> bool {
    let (mut words, mut total) = (0, 0);
    for token in line.split_whitespace() {
        total += 1;
        let core = token.trim_matches(|c: char| ".,;:!?\"'()".contains(c));
        if !core.is_empty()
            && core
                .chars()
                .all(|c| c.is_alphabetic() || c == '-' || c == '\'')
        {
            words += 1;
        }
    }
    words >= 6 && words * 10 >= total * 7
}

/// 注释是写给人看的，选它多半就是想翻译。
fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    ["//", "/*", "* ", "# ", "-- ", "<!--"]
        .iter()
        .any(|p| t.starts_with(p))
}

fn looks_like_code(text: &str) -> bool {
    // 中日韩文字多的是普通文字
    let (mut cjk, mut all) = (0usize, 0usize);
    for c in text.chars().filter(|c| !c.is_whitespace()) {
        all += 1;
        cjk += usize::from(is_cjk(c));
    }
    if cjk * 5 > all {
        return false;
    }
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty() && !is_comment(l))
        .collect();
    match lines.as_slice() {
        [] => false,
        // 只有一行：要两样特征才算，免得把带个括号、带个箭头的句子当成代码；读着像句子的要三样
        [line] => {
            let n = code_signals(line);
            n >= 3 || (n == 2 && !reads_like_prose(line))
        }
        // 多行：一半以上的行像代码
        many => {
            let code = many
                .iter()
                .filter(|l| code_signals(l) >= 1 && !reads_like_prose(l))
                .count();
            code >= 2 && code * 2 >= many.len()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::worth_translating as worth;

    #[test]
    fn prose_is_worth() {
        for text in [
            "Hello",
            "state-of-the-art",
            "COVID-19",
            "The quick brown fox jumps over the lazy dog.",
            "这是一段需要翻译的中文。",
            "苹果(Apple)发布了新系统",
            "Click Settings -> General to change it",
            "See the docs (section 3) for details.",
            "The function foo(bar) returns the number of items in the list; it never fails.",
            "// returns the number of items in the list",
            "# 这里先把缓存清掉\n# 再重新读一遍",
            "git 是一个分布式版本控制系统",
            "First line of a paragraph.\nSecond line of the same paragraph.",
            "Error: connection refused while connecting to the upstream server",
            "let me know if you need anything else from our side",
        ] {
            assert!(worth(text), "该弹：{text}");
        }
    }

    #[test]
    fn links_paths_numbers_are_not() {
        for text in [
            "",
            "12345",
            "3.14 + 2 = 5.14",
            "https://example.com/a/b?c=1",
            "https://例子.com/路径",
            "www.example.com",
            "someone@example.com",
            "example.com/path",
            "https://a.com https://b.com",
            "/Users/cheny/Documents/report final.pdf",
            "~/Library/Application Support",
            "C:\\Program Files\\App\\app.exe",
            "./scripts/release.mjs",
            "report.pdf",
            "v1.2.3",
            "817bd25a0f3c4e1d",
            "550e8400-e29b-41d4-a716-446655440000",
            "user_name",
            "foo.bar()",
        ] {
            assert!(!worth(text), "不该弹：{text}");
        }
    }

    #[test]
    fn code_is_not() {
        for text in [
            "const result = await fetchUser(id);",
            "let mut st = STATE.lock();",
            "git commit -m \"fix parser\"",
            "ls -la",
            "fn main() {\n    println!(\"hi\");\n}",
            "import os\nimport sys\n\ndef main():\n    print(sys.argv)",
            "if (a == b) {\n  return a;\n}",
            "<div class=\"panel\">\n  <span>ok</span>\n</div>",
            "\"name\": \"dato\",\n\"version\": \"0.3.2\",",
            "SELECT id, name FROM users WHERE id = 1;",
        ] {
            assert!(!worth(text), "不该弹：{text}");
        }
    }
}
