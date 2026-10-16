/*
 * DATO OCR 宣传片。
 *
 * 整部片子是一个"时间 → 画面"的纯函数：FILM.seek(t) 把舞台摆成第 t 秒的样子，不依赖真实时钟，
 * 所以 render.mjs 可以一帧一帧精确地截下来，浏览器里打开也能直接播放、拖动预览。
 * 音效的时间点（cue）在搭场景时登记到 FILM.cues，soundtrack.py 照着它合成配乐和音效，画面和声音严格对齐。
 *
 * 节奏：120 BPM，一拍 0.5 秒，一小节 2 秒，转场都落在拍子上。
 */
'use strict';
(() => {
  const Q = new URLSearchParams(location.search);
  const LANG = Q.get('lang') === 'en' ? 'en' : 'zh';
  const RENDER = Q.has('render');
  const W = 1920;
  const H = 1080;
  const FPS = 60;
  const DURATION = 78;

  // ───────────────────────── 数学 ─────────────────────────
  const clamp = (x, a = 0, b = 1) => (x < a ? a : x > b ? b : x);
  const lerp = (a, b, p) => a + (b - a) * p;
  const E = {
    lin: (p) => p,
    inQuad: (p) => p * p,
    outQuad: (p) => 1 - (1 - p) * (1 - p),
    inCubic: (p) => p * p * p,
    outCubic: (p) => 1 - Math.pow(1 - p, 3),
    inOutCubic: (p) => (p < 0.5 ? 4 * p * p * p : 1 - Math.pow(-2 * p + 2, 3) / 2),
    outQuart: (p) => 1 - Math.pow(1 - p, 4),
    inOutQuart: (p) => (p < 0.5 ? 8 * p * p * p * p : 1 - Math.pow(-2 * p + 2, 4) / 2),
    outQuint: (p) => 1 - Math.pow(1 - p, 5),
    inOutSine: (p) => -(Math.cos(Math.PI * p) - 1) / 2,
    outExpo: (p) => (p >= 1 ? 1 : 1 - Math.pow(2, -10 * p)),
    inExpo: (p) => (p <= 0 ? 0 : Math.pow(2, 10 * p - 10)),
    inOutExpo: (p) => (p <= 0 ? 0 : p >= 1 ? 1 : p < 0.5 ? Math.pow(2, 20 * p - 10) / 2 : (2 - Math.pow(2, -20 * p + 10)) / 2),
    outBack: (p) => 1 + 2.70158 * Math.pow(p - 1, 3) + 1.70158 * Math.pow(p - 1, 2),
    outBackSoft: (p) => 1 + 2.2 * Math.pow(p - 1, 3) + 1.2 * Math.pow(p - 1, 2),
    // 带一点回弹的弹簧，p=1 时正好停稳
    spring: (p) => (p >= 1 ? 1 : 1 - Math.exp(-6.5 * p) * Math.cos(9 * p) * (1 - p) - p * Math.exp(-6.5) * 0),
  };
  /** t 在 [a, b] 里的进度，套上缓动 */
  const P = (t, a, b, e = E.lin) => e(clamp((t - a) / (b - a)));
  /** 淡入淡出窗口：[a, a+din] 进，[b-dout, b] 出 */
  const fade = (t, a, b, din = 0.35, dout = 0.35) =>
    Math.min(P(t, a, a + din, E.outCubic), 1 - P(t, b - dout, b, E.inCubic));

  function rng(seed) {
    return () => {
      seed |= 0;
      seed = (seed + 0x6d2b79f5) | 0;
      let r = Math.imul(seed ^ (seed >>> 15), 1 | seed);
      r = (r + Math.imul(r ^ (r >>> 7), 61 | r)) ^ r;
      return ((r ^ (r >>> 14)) >>> 0) / 4294967296;
    };
  }

  // ───────────────────────── DOM ─────────────────────────
  const $ = (s, root = document) => root.querySelector(s);
  const $$ = (s, root = document) => [...root.querySelectorAll(s)];
  const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  function el(html) {
    const t = document.createElement('template');
    t.innerHTML = html.trim();
    return t.content.firstElementChild;
  }
  /** 统一写 transform / opacity / blur */
  function put(e, { x = 0, y = 0, s = 1, sx, sy, r = 0, o, blur } = {}) {
    e.style.transform = `translate(${x.toFixed(2)}px,${y.toFixed(2)}px) rotate(${r.toFixed(3)}deg) scale(${(sx ?? s).toFixed(4)},${(sy ?? s).toFixed(4)})`;
    if (o !== undefined) e.style.opacity = clamp(o).toFixed(4);
    if (blur !== undefined) e.style.filter = blur > 0.05 ? `blur(${blur.toFixed(2)}px)` : 'none';
  }
  /** 把文字拆成逐字的 span；segs 可以是字符串，也可以是 [文字, class] 的数组 */
  function chars(segs) {
    if (typeof segs === 'string') segs = [[segs, '']];
    // 中文逐字出现；英文逐词出现（逐字母太碎，节奏也会被拖慢）
    const split = (text) => (LANG === 'en' ? text.split(/(\s+)/).filter(Boolean) : [...text]);
    return segs
      .map(([text, cls]) => split(text).map((c) => `<span class="ch ${cls || ''}">${/^\s+$/.test(c) ? ' ' : esc(c)}</span>`).join(''))
      .join('');
  }
  /** 逐字出现：上浮 + 去模糊；out 给定时整体淡出 */
  function revealChars(root, t, start, o = {}) {
    const { stagger = 0.03, dur = 0.5, y = 28, blur = 10, out, outDur = 0.3, from = 0, ease = E.outCubic, scale = 0 } = o;
    const cs = root._chars || (root._chars = $$('.ch', root));
    const outP = out === undefined ? 0 : P(t, out, out + outDur, E.inCubic);
    for (let i = 0; i < cs.length; i++) {
      const c = cs[i];
      const a = start + Math.max(0, i - from) * stagger;
      const p = P(t, a, a + dur, ease);
      c.style.opacity = (p * (1 - outP)).toFixed(3);
      const sc = scale ? 1 + scale * (1 - p) : 1;
      c.style.transform = `translateY(${((1 - p) * y - outP * 12).toFixed(2)}px) scale(${sc.toFixed(3)})`;
      const b = (1 - p) * blur + outP * 6;
      c.style.filter = b > 0.05 ? `blur(${b.toFixed(2)}px)` : 'none';
    }
  }

  // ───────────────────────── 文案 ─────────────────────────
  const COPY = {
    zh: {
      s1a: '屏幕上的字，看得见，',
      s1b: '却拿不走。',
      s1tip: '无法选中图片里的文字',
      s2lead: '看到想留下的内容，通常要走四步：',
      s2steps: ['截下来', '认出来', '翻译', '贴过去'],
      s2win: ['截图工具', '在线识字', '翻译网页', '笔记'],
      s2upload: '拖入图片上传',
      s2paste: '粘贴到这里',
      s2pops: [
        ['warn', '上传失败，请重试'],
        ['err', '今日免费次数已用完'],
        ['warn', '请先登录'],
        ['err', '识别结果：Slmpl1c1ty 1s th3…'],
        ['warn', '排版全乱了'],
        ['err', '翻译：简单性是终极的诡辩'],
        ['warn', '网络超时'],
        ['err', '复制失败'],
        ['warn', '请完成人机验证'],
        ['err', '格式不支持'],
        ['warn', '又切错窗口了……'],
        ['err', '广告 · 5 秒后可关闭'],
      ],
      s2count: (n, s) => `切换窗口 ${n} 次 · 用时 0:${String(s).padStart(2, '0')}`,
      s2one: '其实，一步就够了。',
      s3words: ['截图', '识字', '翻译', '剪贴板'],
      s3tag: '一个小工具，一气呵成。',

      s4: { label: '截图', title: '框一下，\n就能用。', sub: '自动识别窗口 · 边缘吸附<br>标注、马赛克，画完还能改' },
      s4hint: 'macOS <b>⌥ 1</b>',
      s4url: 'dato.app/blog/less-is-more',
      s4tag: '设计笔记 · 阅读约 3 分钟',
      s4h1: '少即是多：一个小工具的自我修养',
      s4p1: ['好工具应该在你需要的那一秒出现，', '用完就消失，不留下一点打扰。'],
      s4p2: '截图、识字、翻译、剪贴板，本来就是同一件事的四个步骤。把它们放在一起，事情就会变得很简单。',
      s4mail: '作者：lin@dato.app',
      s4toast: '已复制到剪贴板',
      s4tiles: [
        ['长截图', 'F2', '滚动页面，自动拼成一张长图'],
        ['录 GIF', 'G', '框好区域，直接录成动图'],
        ['贴图', 'P', '钉在屏幕最上层，随手对照'],
        ['瞬间截屏', 'Shift F1', '一碰就消失的菜单也截得到'],
      ],

      s5: { label: '识字', title: '像素，变回文字。', sub: '完全离线，图片不上传 · 识别完自动复制' },
      s5hint: 'macOS <b>⌥ 3</b>',
      s5yes: '这次，选得中了。',
      s5toast: '已识别 · 已复制 42 个字符',
      s5chips: ['离线引擎，在本机识别', '图片不上传', '识别完自动复制'],
      s6: { label: '翻译', title: '译文，就写在原处。', sub: '截图后一键翻译 · 中英混排各翻各的 · 译文原文都能选字' },
      s6hint: 'macOS <b>⌘ T</b>',
      s6zh: '简单是终极的复杂。',
      s6chips: ['译文、原文都能直接复制', '可以指定翻译服务', '已是中文的部分不动'],
      s6b: { label: '划词翻译', title: '选中，就有答案。', sub: '有道 · 必应 · 谷歌 · 腾讯 开箱即用 · 还能直接问 AI' },
      s6bhint: 'macOS <b>⌃⌥ T</b>',
      s6doc: '读书笔记.md',
      s6docLines: ['# 设计原则', '少一点功能，多一点顺手。', ''],
      s6from: '自动检测',
      s6to: '自动（中 ↔ 英）',
      s6default: '默认',
      s6services: [
        ['有道翻译', '有', '#e5483b', '简单是终极的成熟。'],
        ['必应翻译', 'b', 'linear-gradient(135deg,#37b6e9,#2f7fe0)', '简单是终极的复杂。'],
        ['谷歌翻译', 'G', '#fff', '简单是最终的复杂。'],
        ['腾讯翻译', 'T', 'linear-gradient(135deg,#6a7dff,#5b4ef0)', '简单是最终的复杂。'],
      ],
      s6ask: '这句话出自谁？',
      s6askPh: '问 AI…  Enter 发送',
      s6answer: '常被认为出自达·芬奇。意思是：真正高明的设计，看起来往往最简单。',

      s7: { label: '剪贴板', title: '复制过的，都找得回来。', sub: '验证码自动复制 · 分组 · 多端同步 · 历史只存在本机' },
      s7hint: 'macOS <b>⌥ V</b>',
      s7search: '搜索剪贴板…',
      s7tabs: ['剪贴板', '工作', '灵感'],
      s7cards: {
        otp: ['验证码', '刚刚', '482913', '【DATO】你的登录验证码是 482913，5 分钟内有效，请勿告诉他人。', '邮件'],
        text: ['文字', '2 分钟前', 'Simplicity is the ultimate sophistication.', 'DATO OCR · 识字', '42 字符'],
        image: ['图片', '9 分钟前', '880 × 574'],
        color: ['颜色', '14 分钟前', '#2DF5B0', '设计工具'],
        code: ['代码', '26 分钟前', '47 字'],
      },
      s7mail: ['邮件', '刚刚', 'DATO 账户', '【DATO】你的登录验证码是 482913，5 分钟内有效，请勿告诉他人。'],
      s7login: ['登录 DATO', 'lin@dato.app', '验证码', '登录', '登录成功'],
      s7copied: '验证码已自动复制',
      s7sync: ['同一个 Wi-Fi，扫码就连', '也可以用 WebDAV 网盘同步'],
      s7phone: '手机',

      s8words: [
        ['长截图', '滚动页面，自动拼接'],
        ['录 GIF', '框好区域，直接录'],
        ['贴图', '钉在屏幕最上层'],
        ['马赛克', '一抹就糊掉'],
        ['11 个翻译服务', '有道 · 必应 · 谷歌 · 腾讯 · DeepL …'],
        ['问 AI', '带着截图和选中的字提问'],
        ['MCP', '让 Claude、Cursor 查你的剪贴板'],
        ['全都能改', '每一个快捷键、每一种样式'],
      ],
      s8chips: ['瞬间截屏', '窗口识别', '边缘吸附', '标注', '磨砂玻璃', '工具条随你排', '中英混排', 'DeepLX', '百度', '彩云小译', '小牛', '火山', '微软 Azure', '腾讯云', '自定义接口', '分组', '智能推荐', '应用黑名单', '自动清理', 'WebDAV', '扫码同步', '邮箱验证码', 'iPhone 短信', '中文 / English', 'Windows', 'macOS', '截图库', '识字记录'],
      s9: ['识字，在本机完成。', '历史，只留在本机。'],
      s9sub: '截图、识字记录、剪贴板历史从不上传；<br>密钥和邮箱密码加密保存在你的电脑上。',
      s10tag: '截图、识字、翻译、剪贴板 —— 一个小工具，一气呵成。',
      s10chips: ['Windows 10 / 11', 'macOS 13+（Apple 芯片）', '免费', '开源 · GPL-3.0'],
      s10url: 'github.com/Chenfugui69/DATO-OCR',
      s10now: '现在，按下',
      s10toast: '已复制',
      s2btn: ['取消', '确定'],
      s4menu: ['文件', ['新建', '打开…', '最近使用', '导出为 PDF']],
      hello: '你好，',
      s7linked: '✓ 已连接 · 同一个 Wi-Fi',
      s9chips: ['离线识字', '不上传', '加密保存'],
    },
    en: {
      s1a: 'You can see the words, ',
      s1b: 'but you can’t take them.',
      s1tip: 'Text in an image can’t be selected',
      s2lead: 'Keeping something you see usually takes four steps:',
      s2steps: ['Capture', 'Recognize', 'Translate', 'Paste'],
      s2win: ['Snipping tool', 'Online OCR', 'Web translator', 'Notes'],
      s2upload: 'Drop an image to upload',
      s2paste: 'Paste here',
      s2pops: [
        ['warn', 'Upload failed. Try again'],
        ['err', 'Daily free limit reached'],
        ['warn', 'Please sign in first'],
        ['err', 'Result: Slmpl1c1ty 1s th3…'],
        ['warn', 'The layout is a mess'],
        ['err', 'Translation: “Simple is final fancy”'],
        ['warn', 'Network timeout'],
        ['err', 'Copy failed'],
        ['warn', 'Prove you are human'],
        ['err', 'Format not supported'],
        ['warn', 'Wrong window. Again…'],
        ['err', 'Ad · skip in 5s'],
      ],
      s2count: (n, s) => `Switched windows ${n}× · 0:${String(s).padStart(2, '0')}`,
      s2one: 'It only takes one step.',
      s3words: ['Capture', 'Read', 'Translate', 'Clipboard'],
      s3tag: 'One small tool. One smooth move.',

      s4: { label: 'CAPTURE', title: 'Select it.\nDone.', sub: 'Windows detected · edges snap<br>Annotate, blur, adjust anytime' },
      s4hint: 'macOS <b>⌥ 1</b>',
      s4url: 'dato.app/blog/less-is-more',
      s4tag: 'Design notes · 3 min read',
      s4h1: 'Less is more: the craft of a small tool',
      s4p1: ['A good tool shows up the second you need it, ', 'then gets out of your way.'],
      s4p2: 'Capture, recognize, translate, paste: four steps of the same task. Put them together and it all gets simple.',
      s4mail: 'Author: lin@dato.app',
      s4toast: 'Copied to clipboard',
      s4tiles: [
        ['Scrolling', 'F2', 'Scroll the page, get one long image'],
        ['Record GIF', 'G', 'Select an area and hit record'],
        ['Pin', 'P', 'Keep it on top of everything'],
        ['Instant', 'Shift F1', 'Even menus that vanish on touch'],
      ],

      s5: { label: 'RECOGNIZE', title: 'Pixels, back into text.', sub: 'Fully offline, images never uploaded · copied automatically' },
      s5hint: 'macOS <b>⌥ 3</b>',
      s5yes: 'This time, it selects.',
      s5toast: 'Recognized · 42 characters copied',
      s5chips: ['On-device engine', 'Images never uploaded', 'Copied automatically'],
      s6: { label: 'TRANSLATE', title: 'Translated, right in place.', sub: 'One key after capturing · mixed languages line by line · both sides selectable' },
      s6hint: 'macOS <b>⌘ T</b>',
      s6zh: '简单是终极的复杂。',
      s6chips: ['Copy the translation or the original', 'Pick your service', 'Text already translated stays'],
      s6b: { label: 'SELECTION TRANSLATE', title: 'Select it. Get answers.', sub: 'Youdao · Bing · Google · Tencent built in · or just ask AI' },
      s6bhint: 'macOS <b>⌃⌥ T</b>',
      s6doc: 'reading-notes.md',
      s6docLines: ['# Design principles', 'Fewer features. More flow.', ''],
      s6from: 'Auto detect',
      s6to: 'Auto (zh ↔ en)',
      s6default: 'Default',
      s6services: [
        ['Youdao', '有', '#e5483b', '简单是终极的成熟。'],
        ['Bing', 'b', 'linear-gradient(135deg,#37b6e9,#2f7fe0)', '简单是终极的复杂。'],
        ['Google', 'G', '#fff', '简单是最终的复杂。'],
        ['Tencent', 'T', 'linear-gradient(135deg,#6a7dff,#5b4ef0)', '简单是最终的复杂。'],
      ],
      s6ask: 'Who said this?',
      s6askPh: 'Ask AI…  Enter to send',
      s6answer: 'Often attributed to Leonardo da Vinci. The idea: truly great design tends to look the simplest.',

      s7: { label: 'CLIPBOARD', title: 'Copied once, found forever.', sub: 'Codes copied for you · groups · sync · history stays on your machine' },
      s7hint: 'macOS <b>⌥ V</b>',
      s7search: 'Search clipboard…',
      s7tabs: ['Clipboard', 'Work', 'Ideas'],
      s7cards: {
        otp: ['Sign-in code', 'Just now', '482913', '[DATO] Your sign-in code is 482913. Valid for 5 minutes.', 'Mail'],
        text: ['Text', '2 min ago', 'Simplicity is the ultimate sophistication.', 'DATO OCR · Recognize', '42 chars'],
        image: ['Image', '9 min ago', '880 × 574'],
        color: ['Color', '14 min ago', '#2DF5B0', 'Design tool'],
        code: ['Code', '26 min ago', '47 chars'],
      },
      s7mail: ['Mail', 'now', 'DATO Account', '[DATO] Your sign-in code is 482913. Valid for 5 minutes.'],
      s7login: ['Sign in to DATO', 'lin@dato.app', 'Verification code', 'Sign in', 'Signed in'],
      s7copied: 'Code copied automatically',
      s7sync: ['Scan to connect', 'or sync via WebDAV'],
      s7phone: 'Phone',

      s8fs: 128,
      s8words: [
        ['Scrolling', 'Scroll, and it stitches'],
        ['GIF', 'Select an area, hit record'],
        ['Pin', 'Keep it on top'],
        ['Mosaic', 'One swipe to blur'],
        ['11 translators', 'Youdao · Bing · Google · Tencent · DeepL …'],
        ['Ask AI', 'About the text or the screenshot'],
        ['MCP', 'Let Claude or Cursor search your clipboard'],
        ['Your way', 'Every shortcut, every style'],
      ],
      s8chips: ['Instant capture', 'Window detection', 'Edge snapping', 'Annotations', 'Frosted glass', 'Custom toolbar', 'Mixed languages', 'DeepLX', 'Baidu', 'Caiyun', 'Niutrans', 'Volcengine', 'Azure', 'Tencent Cloud', 'Any web API', 'Groups', 'Smart picks', 'App blacklist', 'Auto cleanup', 'WebDAV', 'QR sync', 'Mail codes', 'iPhone SMS', '中文 / English', 'Windows', 'macOS', 'Screenshot library', 'OCR history'],
      s9: ['Recognized locally.', 'Stored locally.'],
      s9sub: 'Screenshots, OCR and clipboard history are never uploaded;<br>API keys and mail passwords are encrypted on your computer.',
      s10tag: 'Capture, read, translate, paste — one small tool, one smooth move.',
      s10chips: ['Windows 10 / 11', 'macOS 13+ (Apple silicon)', 'Free', 'Open source · GPL-3.0'],
      s10url: 'github.com/Chenfugui69/DATO-OCR',
      s10now: 'Now, press',
      s10toast: 'Copied',
      s2btn: ['Cancel', 'OK'],
      s4menu: ['File', ['New', 'Open…', 'Recent', 'Export as PDF']],
      hello: 'Hello, ',
      s7linked: '✓ Connected',
      s9chips: ['Offline OCR', 'Never uploaded', 'Encrypted'],
    },
  };
  const T = COPY[LANG] || COPY.zh;

  // ───────────────────────── 音效 cue ─────────────────────────
  const CUES = [];
  /** 字体加载完、需要量尺寸的初始化放这里 */
  const afterLayout = [];
  const cue = (t, type, extra = {}) => CUES.push({ t: Math.round(t * 1000) / 1000, type, ...extra });

  // ───────────────────────── 场景 ─────────────────────────
  const SCENES = [];
  function scene(id, t0, t1, html, update) {
    const node = el(`<section class="scene" id="${id}">${html}</section>`);
    $('#scenes').appendChild(node);
    const cache = new Map();
    const q = (s) => {
      if (!cache.has(s)) cache.set(s, node.querySelector(s));
      return cache.get(s);
    };
    const qa = (s) => {
      const k = '*' + s;
      if (!cache.has(k)) cache.set(k, [...node.querySelectorAll(s)]);
      return cache.get(k);
    };
    const sc = { id, t0, t1, node, update, q, qa, shown: false };
    SCENES.push(sc);
    return sc;
  }

  // ───────────────────────── 光标 ─────────────────────────
  const CURSORS = {
    arrow: `<svg width="34" height="46" viewBox="0 0 17 23" style="transform:translate(-2px,-2px)"><path d="M1 1v18.5l4.6-4.4 3.1 7 3-1.3-3-6.8h6.3z" fill="#fff" stroke="#111" stroke-width="1.2" stroke-linejoin="round"/></svg>`,
    ibeam: `<svg width="20" height="44" viewBox="-10 -22 20 44"><path d="M-6 -19h3.5q3 0 3 3v32q0 3 -3 3h-3.5M6 -19h-3.5q-3 0 -3 3v32q0 3 3 3h3.5M-4 0h8" fill="none" stroke="#111" stroke-width="5" stroke-linecap="round"/><path d="M-6 -19h3.5q3 0 3 3v32q0 3 -3 3h-3.5M6 -19h-3.5q-3 0 -3 3v32q0 3 3 3h3.5M-4 0h8" fill="none" stroke="#fff" stroke-width="2.2" stroke-linecap="round"/></svg>`,
    cross: `<svg width="56" height="56" viewBox="-28 -28 56 56"><path d="M-24 0h18M6 0h18M0 -24v18M0 6v18" stroke="#000" stroke-opacity=".55" stroke-width="5"/><path d="M-24 0h18M6 0h18M0 -24v18M0 6v18" stroke="#fff" stroke-width="2"/><circle r="2.2" fill="#2df5b0"/></svg>`,
  };
  const cursorEl = $('#cursor');
  let cursorState = null;
  /** 场景在 update 里调用；没人调用的帧光标就藏起来 */
  function setCursor(type, x, y, { o = 1, press = 0 } = {}) {
    cursorState = { type, x, y, o, press };
  }
  function drawCursor() {
    const c = cursorState;
    if (!c || c.o <= 0.001) {
      cursorEl.style.opacity = 0;
      return;
    }
    if (cursorEl._type !== c.type) {
      cursorEl.innerHTML = CURSORS[c.type];
      cursorEl._type = c.type;
      const svg = cursorEl.firstElementChild;
      if (c.type !== 'arrow') svg.style.transform = `translate(-50%,-50%)`;
    }
    cursorEl.style.opacity = c.o;
    const s = 1 - 0.12 * c.press;
    cursorEl.style.transform = `translate(${c.x.toFixed(1)}px,${c.y.toFixed(1)}px) scale(${s})`;
  }
  /** 按下的深度：0 → 1（快速按下）→ 0（回弹） */
  const press = (t, at, hold = 0.1) => (t < at ? 0 : t < at + 0.06 ? (t - at) / 0.06 : t < at + 0.06 + hold ? 1 : 1 - P(t, at + 0.06 + hold, at + 0.28 + hold, E.outCubic));

  // ───────────────────────── 键帽 ─────────────────────────
  function keysHTML(cls, keys, hint, x, y) {
    return `<div class="keys ${cls}" style="left:${x}px;top:${y}px">${keys
      .map((k) => `<div class="key">${k}</div>`)
      .join('<div class="plus">+</div>')}</div>${hint ? `<div class="key-hint ${cls}-hint" style="left:${x + 4}px;top:${y + 124}px">${hint}</div>` : ''}`;
  }
  /** 键帽组入场、在 tp 按下、tOut 退场 */
  function animKeys(sc, cls, t, tIn, tp, tOut = 1e9) {
    const g = sc.q('.' + cls);
    const hint = sc.q('.' + cls + '-hint');
    const pin = P(t, tIn, tIn + 0.5, E.outBack);
    const o = P(t, tIn, tIn + 0.2) * (1 - P(t, tOut, tOut + 0.3));
    put(g, { y: (1 - pin) * 50 - P(t, tOut, tOut + 0.3, E.inCubic) * 20, o });
    if (hint) put(hint, { y: (1 - pin) * 30, o: o * P(t, tIn + 0.2, tIn + 0.6) });
    const d = press(t, tp, 0.14);
    const glow = t > tp ? Math.exp(-(t - tp) * 2.2) : 0;
    for (const k of sc.qa(`.${cls} .key`)) {
      k.style.transform = `translateY(${(d * 8).toFixed(2)}px)`;
      k.style.boxShadow = `0 ${(10 - 8 * d).toFixed(1)}px 0 #15171a, 0 ${(18 - 10 * d).toFixed(1)}px 34px rgba(0,0,0,.55), inset 0 1px 0 rgba(255,255,255,.16), inset 0 0 0 ${(1.5 * glow).toFixed(2)}px rgba(45,245,176,${(0.9 * glow).toFixed(3)}), 0 0 ${(60 * glow).toFixed(1)}px rgba(45,245,176,${(0.45 * glow).toFixed(3)})`;
      k.style.color = glow > 0.02 ? `color-mix(in srgb, #2df5b0 ${(glow * 80).toFixed(0)}%, #eef2f4)` : '#eef2f4';
    }
  }

  // ───────────────────────── 背景 / 颗粒 / 闪光 ─────────────────────────
  const bg = $('#bg');
  const BLOBS = [
    { c: '#1c6f57', w: 1100, x: 1350, y: -250, ax: 120, ay: 80, f: 0.07, ph: 0 },
    { c: '#134a42', w: 1200, x: -350, y: 450, ax: 140, ay: 90, f: 0.05, ph: 2 },
    { c: '#3d5a1c', w: 700, x: 980, y: 640, ax: 160, ay: 70, f: 0.06, ph: 4, k: 0.55 },
    { c: '#0f3a4a', w: 900, x: 520, y: -380, ax: 100, ay: 60, f: 0.04, ph: 1 },
  ];
  for (const b of BLOBS) {
    b.el = el(`<div class="blob" style="width:${b.w}px;height:${b.w}px;background:radial-gradient(circle, ${b.c} 0%, transparent 68%)"></div>`);
    bg.appendChild(b.el);
  }
  /** 背景亮度随段落变化：开场全黑，落拍后亮起 */
  const BG_KEYS = [
    [0, 0], [1.5, 0.15], [7.5, 0.3], [10, 0.45], [12.2, 0.12], [13.9, 0.05], [14.05, 1], [20, 0.85],
    [58, 0.85], [60, 1], [64, 0.55], [68, 1], [76, 0.8], [78, 0],
  ];
  function keyed(keys, t) {
    if (t <= keys[0][0]) return keys[0][1];
    for (let i = 1; i < keys.length; i++) {
      if (t <= keys[i][0]) {
        const [t0, v0] = keys[i - 1];
        const [t1, v1] = keys[i];
        return lerp(v0, v1, E.inOutSine((t - t0) / (t1 - t0)));
      }
    }
    return keys[keys.length - 1][1];
  }
  function drawBg(t) {
    const k = keyed(BG_KEYS, t);
    for (const b of BLOBS) {
      const x = b.x + Math.sin(t * b.f * 2 * Math.PI + b.ph) * b.ax;
      const y = b.y + Math.cos(t * b.f * 1.6 * Math.PI + b.ph) * b.ay;
      put(b.el, { x, y, o: k * 0.7 * (b.k ?? 1) });
    }
  }

  const grain = $('#grain');
  {
    const c = document.createElement('canvas');
    c.width = c.height = 256;
    const g = c.getContext('2d');
    const img = g.createImageData(256, 256);
    const r = rng(7);
    for (let i = 0; i < img.data.length; i += 4) {
      const v = (r() * 255) | 0;
      img.data[i] = img.data[i + 1] = img.data[i + 2] = v;
      img.data[i + 3] = 255;
    }
    g.putImageData(img, 0, 0);
    grain.style.backgroundImage = `url(${c.toDataURL()})`;
  }
  function drawGrain(t) {
    // 颗粒每秒换 15 次而不是每帧都换：看着一样"活"，编码器却省下大半码率（逐帧随机噪声几乎压不动）
    const f = Math.floor(t * 15);
    const r = rng(f * 7919 + 13);
    grain.style.transform = `translate(${(r() * 64 - 32).toFixed(0)}px,${(r() * 64 - 32).toFixed(0)}px)`;
  }

  const FLASHES = [];
  const flashAt = (t, peak, decay = 0.4) => FLASHES.push([t, peak, decay]);
  function drawFlash(t) {
    let v = 0;
    for (const [t0, peak, decay] of FLASHES) if (t >= t0) v += peak * Math.exp(-(t - t0) / (decay / 3));
    $('#flash').style.opacity = clamp(v).toFixed(3);
  }

  // ───────────────────────── 品牌转场：图标里的三条线横扫全屏 ─────────────────────────
  const WIPES = [];
  const wipeEl = $('#wipe');
  const WIPE_ROWS = [1, 3, 2];
  for (let i = 0; i < 3; i++) {
    const dots = Array.from({ length: WIPE_ROWS[i] }, (_, k) => `<i style="position:absolute;top:50px;left:${-320 * (WIPE_ROWS[i] - k)}px;width:260px;height:260px;border-radius:50%;background:${['#e1fd60', '#b4fa77', '#7af78f'][k] || '#e1fd60'}"></i>`).join('');
    wipeEl.appendChild(el(`<div class="abs wipe-row" style="top:${i * 360 - 0}px;height:360px;width:3800px;will-change:transform"><div style="position:absolute;left:0;top:0;height:360px;width:3800px;border-radius:180px;background:linear-gradient(90deg,#e9ff5a,#2df5b0 70%)"></div>${dots}</div>`));
  }
  const wipeAt = (center, dur = 0.95) => {
    WIPES.push([center, dur]);
    cue(center - dur / 2, 'whoosh', { dur });
  };
  function drawWipe(t) {
    const rows = $$('.wipe-row', wipeEl);
    let any = false;
    for (const [c, dur] of WIPES) {
      if (t < c - dur / 2 - 0.1 || t > c + dur / 2 + 0.1) continue;
      any = true;
      rows.forEach((r, i) => {
        const a = c - dur / 2 + (i - 1) * 0.04;
        const p = E.inOutSine(clamp((t - a) / dur));
        r.style.transform = `translateX(${lerp(-3800 - 160, W + 1100, p).toFixed(1)}px)`;
      });
    }
    wipeEl.style.visibility = any ? 'visible' : 'hidden';
  }

  // ───────────────────────── 文字点阵（像素 → 文字） ─────────────────────────
  /** 把几行字按卡片大小排好，画到离屏画布上；返回画布、字号、每行的位置 */
  function layoutText({ w, h, lines, weight = 800, family = 'Inter', maxW, maxFs = 120, gap = 1.12, align = 'left', padX = 90 }) {
    const c = document.createElement('canvas');
    c.width = w;
    c.height = h;
    const g = c.getContext('2d');
    const font = (fs) => `${weight} ${fs}px "${family}", "Noto Sans SC"`;
    g.font = font(100);
    const widest = Math.max(...lines.map((l) => g.measureText(l).width));
    const fs = Math.min(maxFs, Math.floor((100 * (maxW ?? w - padX * 2)) / widest));
    g.font = font(fs);
    const ms = lines.map((l) => g.measureText(l));
    const lh = fs * gap;
    const asc = ms[0].actualBoundingBoxAscent;
    const desc = ms[ms.length - 1].actualBoundingBoxDescent;
    const blockH = asc + lh * (lines.length - 1) + desc;
    const top = (h - blockH) / 2;
    const boxes = lines.map((l, i) => {
      const bw = ms[i].width;
      const x = align === 'center' ? (w - bw) / 2 : padX;
      const base = top + asc + i * lh;
      return { text: l, x, w: bw, base, top: base - fs * 0.8, h: fs * 1.06, mid: base - fs * 0.34 };
    });
    g.fillStyle = '#fff';
    boxes.forEach((b) => g.fillText(b.text, b.x, b.base));
    return { canvas: c, fs, boxes, font: font(fs) };
  }
  /** 对文字画布做半调采样，得到点阵 */
  function halftone(src, step, seed = 1) {
    const { width: w, height: h } = src;
    const d = src.getContext('2d').getImageData(0, 0, w, h).data;
    const r = rng(seed);
    const dots = [];
    for (let y = step / 2; y < h; y += step) {
      for (let x = step / 2; x < w; x += step) {
        let sum = 0;
        let n = 0;
        for (let yy = Math.floor(y - step / 2); yy < y + step / 2; yy += 2) {
          for (let xx = Math.floor(x - step / 2); xx < x + step / 2; xx += 2) {
            sum += d[(yy * w + xx) * 4 + 3];
            n++;
          }
        }
        const v = sum / n / 255;
        if (v > 0.06) dots.push({ x, y, v, rad: step * 0.56 * Math.sqrt(v) + 0.5, rnd: r(), rnd2: r(), rnd3: r() });
      }
    }
    return dots;
  }
  /** 整张卡片的淡淡像素格 */
  function pixelGrid(w, h, step) {
    const c = document.createElement('canvas');
    c.width = w;
    c.height = h;
    const g = c.getContext('2d');
    g.fillStyle = 'rgba(190,255,230,0.09)';
    for (let y = step / 2; y < h; y += step) for (let x = step / 2; x < w; x += step) g.fillRect(x - 0.8, y - 0.8, 1.6, 1.6);
    return c;
  }
  const dotColor = (v, a) => {
    // 亮的点偏白，暗的点偏薄荷绿
    const r = Math.round(lerp(60, 236, v));
    const gg = Math.round(lerp(220, 255, v));
    const b = Math.round(lerp(170, 246, v));
    return `rgba(${r},${gg},${b},${a.toFixed(3)})`;
  };

  // 名言卡片：开场"拿不走"，识字一节"变回文字"，翻译一节"原位翻译"，三处是同一张图
  const CARD = { w: 1240, h: 440, step: 8 };
  const QUOTE = ['Simplicity is the', 'ultimate sophistication.'];
  let quoteLayout = null;
  let quoteDots = null;
  let quoteGrid = null;
  const CARD_BG = `background:
      radial-gradient(ellipse 60% 90% at 85% 10%, rgba(80,170,150,.35), transparent 60%),
      radial-gradient(ellipse 50% 80% at 10% 100%, rgba(120,90,200,.22), transparent 65%),
      linear-gradient(135deg,#123433 0%,#0e1f2b 55%,#191630 100%);`;

  // ═════════════════════════ S1 冷开场：看得见，拿不走（0–8） ═════════════════════════
  const S1 = scene(
    's1', 0, 8.05,
    `<div class="abs s1-seed" style="left:951px;top:531px;width:18px;height:18px;border-radius:50%;background:#2df5b0;box-shadow:0 0 30px #2df5b0,0 0 80px rgba(45,245,176,.6)"></div>
     <div class="abs s1-card" style="left:340px;top:200px;width:${CARD.w}px;height:${CARD.h}px;border-radius:28px;${CARD_BG}box-shadow:0 40px 120px rgba(0,0,0,.55);border:1px solid rgba(255,255,255,.07)"></div>
     <canvas class="abs s1-dots" width="${CARD.w}" height="${CARD.h}" style="left:340px;top:200px"></canvas>
     <div class="abs s1-sel" style="left:0;top:0;height:6px;background:rgba(10,132,255,.0)"></div>
     <div class="abs s1-tip glass" style="left:0;top:0;padding:14px 22px;border-radius:14px;font:600 24px var(--sans);display:flex;gap:12px;align-items:center;white-space:nowrap">
       <svg width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="#ff453a" stroke-width="2.4" stroke-linecap="round"><circle cx="12" cy="12" r="9.5"/><path d="M5.3 5.3l13.4 13.4"/></svg>${esc(T.s1tip)}</div>
     <div class="caption s1-cap" style="top:740px;font-size:62px"><span class="s1-a">${chars([[T.s1a, '']])}</span><span class="s1-b">${chars([[T.s1b, '']])}</span></div>`,
    (t, sc) => {
      const seed = sc.q('.s1-seed');
      const pulse = 1 + 0.18 * Math.sin(t * Math.PI * 2 * 1.0);
      put(seed, { s: pulse * P(t, 0.1, 0.5, E.outBack) * (1 - P(t, 0.8, 1.05, E.inCubic)) + P(t, 0.8, 1.05) * 0, o: 1 - P(t, 0.9, 1.1) });

      const card = sc.q('.s1-card');
      const outP = P(t, 7.15, 7.95, E.inCubic);
      put(card, { s: lerp(0.96, 1, P(t, 1.4, 3, E.outCubic)) * (1 - 0.06 * outP), o: P(t, 1.5, 2.6) * (1 - outP) });

      // 点阵：从中心那个点里涌出来，落位成一句话
      const cv = sc.q('.s1-dots');
      const g = cv.getContext('2d');
      g.clearRect(0, 0, CARD.w, CARD.h);
      g.globalAlpha = P(t, 1.3, 2.6) * (1 - outP);
      g.drawImage(quoteGrid, 0, 0);
      g.globalAlpha = 1;
      const sx = 960 - 340;
      const sy = 540 - 200;
      const shake = P(t, 4.62, 4.68) * (1 - P(t, 4.7, 5.2));
      for (const d of quoteDots) {
        const a0 = 0.85 + d.delay;
        const p = P(t, a0, a0 + 0.95, E.outExpo);
        if (p <= 0) continue;
        const ang = (1 - p) * (2.2 + d.rnd2 * 2) * (d.rnd > 0.5 ? 1 : -1);
        const dx = d.x - sx;
        const dy = d.y - sy;
        const cos = Math.cos(ang);
        const sin = Math.sin(ang);
        let x = sx + (dx * cos - dy * sin) * p;
        let y = sy + (dx * sin + dy * cos) * p;
        x += Math.sin(t * 90 + d.rnd * 40) * 3 * shake;
        // 退场：向外飘散
        if (outP > 0) {
          x += (d.x - sx) * outP * (0.4 + d.rnd2);
          y += (d.y - sy) * outP * (0.4 + d.rnd3) - 60 * outP * d.rnd;
        }
        const a = clamp(p * 1.4) * (1 - outP);
        if (a <= 0.003) continue;
        g.fillStyle = dotColor(d.v, a);
        g.beginPath();
        g.arc(x, y, d.rad * (0.35 + 0.65 * p), 0, Math.PI * 2);
        g.fill();
      }

      // 光标试着拖选这句话 —— 选不中
      const L = quoteLayout.boxes;
      const p0 = { x: 340 + L[0].x - 8, y: 200 + L[0].mid };
      const p1 = { x: 340 + L[1].x + L[1].w + 8, y: 200 + L[1].mid };
      if (t > 3.1 && t < 6) {
        let x, y;
        if (t < 3.75) {
          const p = P(t, 3.1, 3.75, E.inOutCubic);
          x = lerp(1640, p0.x, p);
          y = lerp(1010, p0.y, p);
        } else {
          const p = P(t, 3.85, 4.6, E.inOutCubic);
          x = lerp(p0.x, p1.x, p);
          y = lerp(p0.y, p1.y, p);
        }
        const o = P(t, 3.1, 3.3) * (1 - P(t, 5.4, 5.8));
        setCursor('ibeam', x, y, { o, press: t > 3.8 && t < 4.62 ? 1 : 0 });
        const tip = sc.q('.s1-tip');
        const tp = P(t, 4.62, 4.85, E.outBack);
        const wob = Math.sin((t - 4.62) * 50) * 8 * (1 - P(t, 4.62, 5.0));
        tip.style.left = `${p1.x - 200}px`;
        tip.style.top = `${p1.y + 44}px`;
        put(tip, { x: wob, y: (1 - tp) * 16, s: 0.9 + 0.1 * tp, o: P(t, 4.62, 4.75) * (1 - P(t, 5.7, 6)) });
      } else {
        sc.q('.s1-tip').style.opacity = 0;
      }

      revealChars(sc.q('.s1-a'), t, 3.0, { stagger: 0.04, dur: 0.6, out: 7.2, outDur: 0.5 });
      revealChars(sc.q('.s1-b'), t, 5.25, { stagger: Math.min(0.09, 0.5 / sc.q('.s1-b').children.length), dur: 0.7, y: 0, blur: 16, scale: 0.35, out: 7.2, outDur: 0.5 });
      sc.q('.s1-a').style.color = 'var(--ink2)';
    },
  );
  cue(0.1, 'seed');
  cue(0.85, 'swell', { dur: 2.0 });
  cue(3.8, 'click');
  cue(4.62, 'error');
  cue(5.25, 'hit', { soft: 1 });
  cue(7.2, 'scatter', { dur: 0.8 });

  // ═════════════════════════ S2 四步、四个工具，越来越乱（8–14） ═════════════════════════
  const NODES = [
    { x: 150, y: 150, r: -4 },
    { x: 650, y: 470, r: 3 },
    { x: 1110, y: 130, r: -2.5 },
    { x: 1450, y: 450, r: 4 },
  ];
  const NW = 340;
  const NH = 236;
  const nodeBody = [
    `<div style="position:relative;height:150px;margin:20px;border-radius:10px;background:linear-gradient(135deg,#2c3a40,#232a33)"><div style="position:absolute;left:40px;top:28px;width:190px;height:90px;border:2.5px dashed #9fb3c8;border-radius:4px"></div><div style="position:absolute;right:16px;bottom:10px;font-size:34px">✂️</div></div>`,
    `<div style="height:150px;margin:20px;border-radius:12px;border:2.5px dashed rgba(255,255,255,.22);display:grid;place-items:center;align-content:center;gap:10px;color:var(--ink2);font:600 18px var(--sans)"><svg width="34" height="34" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M12 16V4M7 9l5-5 5 5M4 20h16"/></svg>${esc(T.s2upload)}<div style="width:200px;height:8px;border-radius:4px;background:rgba(255,255,255,.1)"><div class="s2-prog" style="width:62%;height:100%;border-radius:4px;background:#5aa9ff"></div></div></div>`,
    `<div style="display:flex;gap:14px;margin:20px;height:150px;align-items:center"><div style="flex:1"><div class="ln" style="width:90%"></div><div class="ln"></div><div class="ln" style="width:70%"></div><div class="ln" style="width:80%"></div></div><div style="font:700 26px var(--sans);color:var(--ink2)">⇄</div><div style="flex:1"><div class="ln" style="width:80%"></div><div class="ln" style="width:95%"></div><div class="ln" style="width:60%"></div><div class="ln" style="width:75%"></div></div></div>`,
    `<div style="margin:20px;height:150px;background:#f3efe4;border-radius:10px;padding:16px 18px;color:#8a8478;font:500 18px var(--sans)"><div class="ln" style="background:#d9d3c4;width:85%"></div><div class="ln" style="background:#d9d3c4;width:60%"></div><div style="margin-top:18px;display:flex;align-items:center;gap:4px">${esc(T.s2paste)}<span class="s2-caret" style="display:inline-block;width:2.5px;height:24px;background:#2a7fff"></span></div></div>`,
  ];
  const popR = rng(42);
  const POPS = T.s2pops.map(([kind, text], i) => ({
    kind,
    text,
    t: 10.0 + i * 0.16,
    x: 70 + popR() * 1380,
    y: 60 + popR() * 600,
    r: (popR() - 0.5) * 14,
  }));
  const S2 = scene(
    's2', 7.75, 14.05,
    `<div class="abs s2-all" style="width:${W}px;height:${H}px">
       <svg class="abs" width="${W}" height="${H}" style="pointer-events:none"><path class="s2-path" fill="none" stroke="rgba(45,245,176,.55)" stroke-width="3" stroke-linecap="round" stroke-dasharray="2 12"/></svg>
       ${NODES.map((n, i) => `<div class="abs s2-node" style="left:${n.x}px;top:${n.y}px;width:${NW}px;height:${NH}px"><div class="win" style="inset:0"><div class="win-bar"><span class="d"></span><span class="d"></span><span class="d"></span>${esc(T.s2win[i])}</div>${nodeBody[i]}</div><div style="position:absolute;right:-18px;top:-18px;width:48px;height:48px;border-radius:50%;background:var(--mint);color:#06130d;display:grid;place-items:center;font:800 24px var(--sans);box-shadow:0 6px 20px rgba(45,245,176,.4)">${i + 1}</div></div>`).join('')}
       ${POPS.map((p) => `<div class="win s2-pop" style="left:${p.x.toFixed(0)}px;top:${p.y.toFixed(0)}px;padding:18px 22px 16px;min-width:300px;background:#25282d"><div style="display:flex;gap:12px;align-items:center;font:600 21px var(--sans)"><span style="width:30px;height:30px;border-radius:50%;flex:none;display:grid;place-items:center;font:800 18px var(--sans);color:#111;background:${p.kind === 'warn' ? '#ffd60a' : '#ff6b5e'}">${p.kind === 'warn' ? '!' : '×'}</span>${esc(p.text)}</div><div style="margin-top:14px;display:flex;justify-content:flex-end;gap:10px"><span style="padding:6px 16px;border-radius:8px;background:rgba(255,255,255,.08);font:600 16px var(--sans);color:var(--ink2)">${esc(T.s2btn[0])}</span><span style="padding:6px 16px;border-radius:8px;background:#3a7bd5;font:600 16px var(--sans)">${esc(T.s2btn[1])}</span></div></div>`).join('')}
       <div class="abs s2-count chip" style="left:auto;right:70px;top:46px;font-family:var(--mono);font-size:21px"></div>
       <div class="caption s2-lead" style="top:812px;font-size:30px;font-weight:500;color:var(--ink2)">${chars(T.s2lead)}</div>
       <div class="caption s2-steps" style="top:872px;font-size:62px;font-weight:900">${T.s2steps.map((s, i) => `<span class="s2-step" style="display:inline-block">${i ? '<span style="color:var(--ink3);font-weight:500;margin:0 26px">→</span>' : ''}${esc(s)}</span>`).join('')}</div>
     </div>
     <div class="abs s2-dot" style="left:950px;top:530px;width:20px;height:20px;border-radius:50%;background:#2df5b0;box-shadow:0 0 30px #2df5b0,0 0 90px rgba(45,245,176,.7)"></div>
     <div class="caption s2-one" style="top:600px;font-size:54px">${chars(T.s2one)}</div>`,
    (t, sc) => {
      const all = sc.q('.s2-all');
      const freeze = P(t, 12.0, 12.2);
      all.style.filter = freeze > 0 ? `grayscale(${freeze}) brightness(${1 - 0.35 * freeze})` : 'none';
      const implode = (cx, cy, delay = 0) => {
        const p = P(t, 12.3 + delay, 12.95, E.inExpo);
        return { dx: (960 - cx) * p, dy: (540 - cy) * p, s: 1 - p, p };
      };

      // 四个工具窗口依次弹出，光标在它们之间来回跑
      const centers = NODES.map((n) => ({ x: n.x + NW / 2, y: n.y + NH / 2 }));
      sc.qa('.s2-node').forEach((node, i) => {
        const a = 8.0 + i * 0.5;
        const p = P(t, a, a + 0.45, E.outBack);
        const im = implode(centers[i].x, centers[i].y, i * 0.04);
        const jit = P(t, 10, 10.3) * (1 - freeze) * Math.sin(t * 37 + i) * 3;
        put(node, { x: im.dx + jit, y: (1 - p) * 40 + im.dy, s: (0.6 + 0.4 * p) * im.s, r: NODES[i].r * (1 + im.p * 6), o: P(t, a, a + 0.15) });
      });
      // 连线：画到哪儿光标就跑到哪儿
      const path = sc.q('.s2-path');
      if (!path._len) {
        const c = centers;
        path.setAttribute('d', `M${c[0].x} ${c[0].y} C ${c[0].x + 300} ${c[0].y + 40}, ${c[1].x - 300} ${c[1].y - 60}, ${c[1].x} ${c[1].y} S ${c[2].x - 250} ${c[2].y + 80}, ${c[2].x} ${c[2].y} S ${c[3].x - 200} ${c[3].y - 120}, ${c[3].x} ${c[3].y}`);
        path._len = path.getTotalLength();
        // 每个节点在路径上的长度位置
        path._at = [0];
        let acc = 0;
        let k = 1;
        for (let s = 0; s <= path._len && k < 4; s += 2) {
          const pt = path.getPointAtLength(s);
          if (Math.hypot(pt.x - c[k].x, pt.y - c[k].y) < 3) {
            path._at.push(s);
            k++;
          }
          acc = s;
        }
        while (path._at.length < 4) path._at.push(acc);
      }
      // 光标到达每个节点的时间
      const arrive = [8.3, 8.8, 9.3, 9.8];
      let len = 0;
      if (t >= arrive[0]) {
        for (let i = 0; i < 3; i++) {
          if (t < arrive[i + 1]) {
            len = lerp(path._at[i], path._at[i + 1], P(t, arrive[i] + 0.08, arrive[i + 1], E.inOutCubic));
            break;
          }
          len = path._at[3];
        }
      }
      const pathO = 1 - P(t, 12.3, 12.6);
      path.style.opacity = pathO;
      // 虚线不能直接用 dashoffset 实现"画出来"，所以把 dasharray 拼成只画到 len 为止
      path.setAttribute('stroke-dasharray', makeDash(len, path._len));

      let cx, cy;
      if (t < arrive[0]) {
        const p = P(t, 7.9, arrive[0], E.inOutCubic);
        cx = lerp(980, centers[0].x, p);
        cy = lerp(1000, centers[0].y + 30, p);
      } else if (t < 10) {
        const pt = path.getPointAtLength(Math.max(0, len));
        cx = pt.x;
        cy = pt.y + 30;
      } else {
        // 乱成一团：光标在弹窗之间乱点
        const i = Math.min(POPS.length - 1, Math.floor((t - 10) / 0.16));
        const pa = i > 0 ? POPS[i - 1] : { x: centers[3].x - 150, y: centers[3].y - 30 };
        const pb = POPS[i];
        const p = P(t, 10 + i * 0.16, 10 + i * 0.16 + 0.14, E.inOutCubic);
        cx = lerp(pa.x + 260, pb.x + 260, p);
        cy = lerp(pa.y + 90, pb.y + 90, p);
      }
      const clk = arrive.reduce((m, a) => Math.max(m, press(t, a, 0.06)), 0);
      const curImp = P(t, 12.3, 12.95, E.inExpo);
      setCursor('arrow', lerp(cx, 960, curImp), lerp(cy, 540, curImp), { o: P(t, 7.9, 8.1) * (1 - P(t, 12.6, 12.9)), press: clk });

      // 弹窗
      sc.qa('.s2-pop').forEach((el2, i) => {
        const pp = POPS[i];
        const p = P(t, pp.t, pp.t + 0.28, E.outBack);
        const im = implode(pp.x + 170, pp.y + 60, 0.02 * (i % 4));
        put(el2, { x: im.dx, y: im.dy, s: (0.5 + 0.5 * p) * im.s, r: pp.r * (1 + im.p * 4), o: P(t, pp.t, pp.t + 0.08) });
      });
      // 进度条在"上传中"慢慢走
      sc.q('.s2-prog').style.width = `${(20 + 50 * P(t, 8.6, 11.5)).toFixed(1)}%`;
      sc.q('.s2-caret').style.opacity = Math.floor(t * 2.5) % 2 ? 0 : 1;

      // 计数器
      const clicks = arrive.filter((a) => t >= a).length + POPS.filter((p) => t >= p.t).length;
      const secs = Math.round(lerp(0, 47, P(t, 8, 12, E.inQuad)));
      const cnt = sc.q('.s2-count');
      if (cnt._v !== clicks + ':' + secs) {
        cnt.textContent = T.s2count(clicks, secs);
        cnt._v = clicks + ':' + secs;
      }
      const cntIm = implode(1700, 70);
      put(cnt, { x: cntIm.dx, y: cntIm.dy, s: cntIm.s, o: P(t, 8.2, 8.5) });

      // 底部字幕
      const capIm = P(t, 12.3, 12.9, E.inExpo);
      revealChars(sc.q('.s2-lead'), t, 8.0, { stagger: 0.02, dur: 0.4 });
      put(sc.q('.s2-lead'), { y: -capIm * 300, s: 1 - capIm, o: 1 - capIm });
      sc.qa('.s2-step').forEach((s, i) => {
        const a = 8.0 + i * 0.5;
        const p = P(t, a, a + 0.4, E.outBack);
        put(s, { y: (1 - p) * 30, s: 0.8 + 0.2 * p, o: P(t, a, a + 0.15) });
      });
      put(sc.q('.s2-steps'), { y: -capIm * 360, s: 1 - capIm, o: 1 - capIm });

      // 一切被吸进一个点
      const dot = sc.q('.s2-dot');
      const dp = P(t, 12.85, 13.05, E.outBack);
      const breathe = 1 + 0.12 * Math.sin((t - 12.9) * Math.PI * 2);
      put(dot, { s: dp * breathe * (1 + 0.6 * P(t, 13.85, 14.0, E.inExpo)), o: dp });
      revealChars(sc.q('.s2-one'), t, 13.05, { stagger: 0.05, dur: 0.45, out: 13.78, outDur: 0.18 });
    },
  );
  function makeDash(len, total) {
    // 2px 实 + 12px 空的虚线，只画到 len：把超出部分用一段很长的空白盖住
    if (len <= 0) return `0 ${total + 10}`;
    const n = Math.floor(len / 14);
    const parts = [];
    for (let i = 0; i < n; i++) parts.push(2, 12);
    parts.push(Math.min(2, len - n * 14), total + 10);
    return parts.join(' ');
  }
  [8.0, 8.5, 9.0, 9.5].forEach((a, i) => cue(a, 'pop', { n: i }));
  [8.3, 8.8, 9.3, 9.8].forEach((a) => cue(a, 'click'));
  POPS.forEach((p, i) => cue(p.t, 'pop', { n: 4 + i, harsh: 1 }));
  cue(10.0, 'riser', { dur: 2.0 });
  cue(12.0, 'freeze');
  cue(12.3, 'implode', { dur: 0.65 });
  cue(12.95, 'seed');

  // ═════════════════════════ S3 落拍：图标长出来（14–20） ═════════════════════════
  const LOGO_ROWS = [
    [352, 1, 760],
    [512, 3, 664],
    [672, 2, 728],
  ];
  function logoSVG(cls, size, { base = true, full = false } = {}) {
    const id = cls + '-g';
    let dots = '';
    let lines = '';
    LOGO_ROWS.forEach(([y, n, end], ri) => {
      for (let i = 0; i < n; i++) dots += `<circle class="${cls}-dot" data-r="${ri}" data-i="${i}" cx="${272 + i * 104}" cy="${y}" r="38" fill="url(#${id})" opacity="${0.38 + 0.2 * i}"/>`;
      const x1 = 272 + n * 104;
      lines += `<line class="${cls}-line" data-end="${end}" x1="${x1}" y1="${y}" x2="${full ? end : x1}" y2="${y}" stroke="url(#${id})" stroke-width="76" stroke-linecap="round"/>`;
    });
    return `<svg class="abs ${cls}" width="${size}" height="${size}" viewBox="192 192 640 640">
      <defs>
        <linearGradient id="${id}" gradientUnits="userSpaceOnUse" x1="232" y1="0" x2="800" y2="0"><stop offset="0" stop-color="#E9FF5A"/><stop offset="1" stop-color="#2DF5B0"/></linearGradient>
        <linearGradient id="${cls}-base" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#2a2f36"/><stop offset=".55" stop-color="#191c21"/><stop offset="1" stop-color="#111316"/></linearGradient>
        <radialGradient id="${cls}-hl" cx=".3" cy=".22" r=".55"><stop offset="0" stop-color="#fff" stop-opacity=".09"/><stop offset="1" stop-color="#fff" stop-opacity="0"/></radialGradient>
      </defs>
      ${base ? `<g class="${cls}-base"><rect x="196" y="196" width="632" height="632" rx="148" fill="url(#${cls}-base)" stroke="rgba(255,255,255,.12)" stroke-width="3"/><rect x="196" y="196" width="632" height="632" rx="148" fill="url(#${cls}-hl)"/></g>` : ''}
      ${dots}${lines}
    </svg>`;
  }
  /** 图标生长动画：点从 (fx,fy) 飞到位，线从点里伸出来 */
  function animLogo(sc, cls, t, t0, { from = [512, 512], baseAt = 0.3, lineAt = 0.32 } = {}) {
    sc.qa(`.${cls}-dot`).forEach((d, k) => {
      const ri = +d.dataset.r;
      const i = +d.dataset.i;
      const tx = 272 + i * 104;
      const ty = LOGO_ROWS[ri][0];
      const a = t0 + k * 0.035;
      const p = P(t, a, a + 0.45, E.outBack);
      d.setAttribute('cx', lerp(from[0], tx, p).toFixed(2));
      d.setAttribute('cy', lerp(from[1], ty, p).toFixed(2));
      d.setAttribute('r', (38 * clamp(p * 1.3)).toFixed(2));
    });
    sc.qa(`.${cls}-line`).forEach((l, ri) => {
      const x1 = +l.getAttribute('x1');
      const end = +l.dataset.end;
      const a = t0 + lineAt + ri * 0.07;
      const p = P(t, a, a + 0.55, E.outExpo);
      l.setAttribute('x2', lerp(x1, end, p).toFixed(2));
      l.style.opacity = P(t, a - 0.05, a + 0.02);
    });
    const base = sc.q(`.${cls}-base`);
    if (base) {
      const p = P(t, t0 + baseAt, t0 + baseAt + 0.6, E.outCubic);
      base.setAttribute('transform', `translate(512 512) scale(${(0.82 + 0.18 * p).toFixed(4)}) translate(-512 -512)`);
      base.style.opacity = p;
    }
  }

  const S3 = scene(
    's3', 13.98, 20.02,
    `<div class="abs s3-ring" style="left:960px;top:540px;width:0;height:0"><div style="position:absolute;left:-50px;top:-50px;width:100px;height:100px;border-radius:50%;border:3px solid #2df5b0"></div></div>
     ${logoSVG('s3-logo', 420)}
     <div class="abs s3-word" style="left:0;top:0;font:800 150px/1 var(--sans);letter-spacing:-0.02em;white-space:nowrap">DATO OCR</div>
     <div class="abs s3-words" style="left:0;top:0;font:700 42px/1 var(--sans);white-space:nowrap;display:flex;gap:30px;align-items:center">${T.s3words.map((w, i) => `${i ? '<i class="s3-sep" style="width:9px;height:9px;border-radius:50%;background:var(--ink3)"></i>' : ''}<span class="s3-w">${esc(w)}</span>`).join('')}</div>
     <div class="caption s3-tag" style="top:742px;font-size:46px;font-weight:700;color:var(--ink)">${chars(T.s3tag)}</div>`,
    (t, sc) => {
      const ring = sc.q('.s3-ring > div');
      const rp = P(t, 14.0, 14.9, E.outCubic);
      ring.style.transform = `scale(${(0.2 + rp * 13).toFixed(3)})`;
      ring.style.opacity = (1 - rp) * (t >= 14 ? 1 : 0);
      ring.style.borderWidth = `${(3 / (0.2 + rp * 13)) * 4}px`;

      const logo = sc.q('.s3-logo');
      animLogo(sc, 's3-logo', t, 14.02);
      // 先在正中，然后让到左边给字标腾地方
      const mv = P(t, 15.55, 16.35, E.inOutQuart);
      const size = lerp(420, 250, mv);
      const cx = lerp(960, 560, mv);
      const cy = lerp(540, 452, mv);
      logo.setAttribute('width', size);
      logo.setAttribute('height', size);
      const glow = 0.55 + 0.45 * Math.exp(-Math.max(0, t - 14.4) * 1.5);
      logo.style.filter = `drop-shadow(0 0 ${(40 * glow).toFixed(1)}px rgba(45,245,176,${(0.35 * glow).toFixed(3)})) drop-shadow(0 30px 60px rgba(0,0,0,.5))`;
      put(logo, { x: cx - size / 2, y: cy - size / 2, s: lerp(0.6, 1, P(t, 14.0, 14.5, E.outBack)), o: 1 - P(t, 19.7, 19.9) });

      const word = sc.q('.s3-word');
      const wp = P(t, 15.95, 16.7, E.outQuart);
      word.style.clipPath = `inset(-20px ${((1 - wp) * 100).toFixed(2)}% -20px 0)`;
      put(word, { x: 730 + (1 - wp) * -40, y: 366, o: P(t, 15.95, 16.2) });
      word.style.letterSpacing = `${lerp(0.08, -0.02, wp).toFixed(4)}em`;

      const words = sc.q('.s3-words');
      put(words, { x: 738, y: 548, o: P(t, 16.3, 16.5) });
      sc.qa('.s3-w').forEach((w, i) => {
        const a = 16.5 + i * 0.5;
        const p = P(t, a, a + 0.35, E.outBack);
        const lit = Math.exp(-Math.max(0, t - a) * 2.5);
        w.style.opacity = (0.25 + 0.75 * P(t, a, a + 0.1)).toFixed(3);
        w.style.transform = `translateY(${((1 - p) * 14).toFixed(1)}px)`;
        w.style.color = t >= a ? `color-mix(in srgb, #2df5b0 ${(lit * 100).toFixed(0)}%, #f4f7f8)` : 'var(--ink3)';
        w.style.textShadow = t >= a ? `0 0 ${(30 * lit).toFixed(1)}px rgba(45,245,176,${(0.8 * lit).toFixed(3)})` : 'none';
      });
      revealChars(sc.q('.s3-tag'), t, 18.3, { stagger: 0.05, dur: 0.5 });
    },
  );
  cue(14.0, 'drop');
  flashAt(14.0, 0.5, 0.6);
  [16.5, 17.0, 17.5, 18.0].forEach((a, i) => cue(a, 'blip', { n: i }));
  cue(15.95, 'shimmer', { dur: 0.8 });
  wipeAt(20.0, 0.95);

  // ───────────────────────── 共用组件 ─────────────────────────
  const ICON = {
    rect: '<rect x="4" y="5" width="16" height="14" rx="2"/>',
    ellipse: '<ellipse cx="12" cy="12" rx="9" ry="7"/>',
    arrow: '<path d="M6 18 18 6M9 6h9v9"/>',
    pen: '<path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z"/>',
    mosaic: '<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M3 9h18M3 15h18M9 3v18M15 3v18"/>',
    text: '<path d="M5 7V5h14v2M9 19h6M12 5v14"/>',
    translate: '<path d="m5 8 6 6M4 14l6-6 2-3M2 5h12M7 2h1M22 22l-5-10-5 10M14 18h6"/>',
    ocr: '<path d="M3 7V5a2 2 0 0 1 2-2h2M17 3h2a2 2 0 0 1 2 2v2M21 17v2a2 2 0 0 1-2 2h-2M7 21H5a2 2 0 0 1-2-2v-2M7 8h8M7 12h10M7 16h6"/>',
    gif: '<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M10 10H8v4h2v-2M13 10v4M16 14v-4h2M16 12h1.5"/>',
    pin: '<path d="M12 17v5M9 10.8a2 2 0 0 1-1.1 1.8l-1.8.9A2 2 0 0 0 5 15.2V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.8a2 2 0 0 0-1.1-1.8l-1.8-.9A2 2 0 0 1 15 10.8V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/>',
    save: '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M7 10l5 5 5-5M12 15V3"/>',
    copy: '<rect x="8" y="8" width="13" height="13" rx="2"/><path d="M4 16a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2"/>',
    x: '<path d="M18 6 6 18M6 6l12 12"/>',
    check: '<path d="M20 6 9 17l-5-5"/>',
    mail: '<rect x="2" y="4" width="20" height="16" rx="2"/><path d="m22 7-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 7"/>',
    lock: '<rect x="3" y="11" width="18" height="11" rx="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>',
    wifiOff: '<path d="M12 20h.01M8.5 16.4a5 5 0 0 1 7 0M2 8.8a15 15 0 0 1 4.2-2.6M10.7 5c4-.4 8.1.9 11.3 3.8M16.9 11.3a10 10 0 0 1 2.2 1.7M5 13a10 10 0 0 1 5.2-2.8M2 2l20 20"/>',
    phone: '<rect x="5" y="2" width="14" height="20" rx="3"/><path d="M11 18h2"/>',
    search: '<circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/>',
    sparkles: '<path d="M10 3.5 11.6 8a2 2 0 0 0 1.3 1.3l4.6 1.6-4.6 1.6a2 2 0 0 0-1.3 1.3L10 18.4l-1.6-4.6a2 2 0 0 0-1.3-1.3L2.5 10.9l4.6-1.6A2 2 0 0 0 8.4 8zM19 2v4M21 4h-4M20 16v3M21.5 17.5h-3"/>',
    up: '<path d="m5 12 7-7 7 7M12 19V5"/>',
    down: '<path d="m6 9 6 6 6-6"/>',
    swap: '<path d="m21 16-4 4-4-4M17 20V4M3 8l4-4 4 4M7 4v16"/>',
    shield: '<path d="M20 13c0 5-3.5 7.5-7.7 9a1 1 0 0 1-.6 0C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.2-2.7a1.2 1.2 0 0 1 1.6 0C14.5 3.8 17 5 19 5a1 1 0 0 1 1 1z"/><path d="m9 12 2 2 4-4"/>',
    github: '<path d="M15 22v-4a4.8 4.8 0 0 0-1-3.5c3 0 6-2 6-5.5.1-1.3-.3-2.5-1-3.5.3-1.2.3-2.4 0-3.5 0 0-1 0-3 1.5-2.6-.5-5.4-.5-8 0C6 2 5 2 5 2c-.3 1.2-.3 2.4 0 3.5A5.4 5.4 0 0 0 4 9c0 3.5 3 5.5 6 5.5-.4.5-.7 1-.8 1.6-.2.6-.3 1.3-.2 1.9v4M9 18c-4.5 2-5-2-7-2"/>',
    more: '<circle cx="5" cy="12" r="1.2"/><circle cx="12" cy="12" r="1.2"/><circle cx="19" cy="12" r="1.2"/>',
    zap: '<path d="M13 2 3 14h9l-1 8 10-12h-9l1-8z"/>',
    scroll: '<path d="M12 4v16M8 16l4 4 4-4M8 8l4-4 4 4"/>',
  };
  const icon = (k, size = 24, color = 'currentColor', sw = 2) =>
    `<svg width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="${color}" stroke-width="${sw}" stroke-linecap="round" stroke-linejoin="round">${ICON[k]}</svg>`;

  const TOOLBAR = [['rect', 'ellipse', 'arrow', 'pen', 'mosaic', 'text'], ['translate', 'ocr', 'gif', 'pin'], ['save', 'copy', 'x', 'check']];
  function toolbarHTML(cls) {
    return `<div class="abs ${cls} glass" style="display:flex;align-items:center;gap:4px;padding:6px;border-radius:16px">${TOOLBAR.map((grp) =>
      grp.map((k) => `<div class="tb-btn" data-k="${k}" style="width:46px;height:46px;border-radius:11px;display:grid;place-items:center;color:${k === 'check' ? '#fff' : 'rgba(240,244,247,.86)'};background:${k === 'check' ? '#0a84ff' : 'transparent'}">${icon(k, 24)}</div>`).join(''),
    ).join('<i style="width:1px;height:26px;background:rgba(255,255,255,.14);margin:0 4px"></i>')}</div>`;
  }

  function hdrHTML(cls, num, h, extra = '') {
    return `<div class="hdr ${cls}" ${extra}><div class="hdr-label"><i></i>${num} / ${esc(h.label)}</div><div class="hdr-title">${h.title.split('\n').map((l) => chars(l)).join('<br>')}</div><div class="hdr-sub">${h.sub}</div></div>`;
  }
  function animHdr(sc, cls, t, tIn, tOut = 1e9) {
    const root = sc.q('.' + cls);
    const outP = P(t, tOut, tOut + 0.45, E.inCubic);
    put(root, { x: -outP * 60, o: 1 - outP });
    const lab = root.firstElementChild;
    const lp = P(t, tIn, tIn + 0.5, E.outCubic);
    put(lab, { x: (1 - lp) * -30, o: lp });
    revealChars(root.children[1], t, tIn + 0.1, { stagger: 0.04, dur: 0.55, y: 40, blur: 12 });
    const sp = P(t, tIn + 0.45, tIn + 0.95, E.outCubic);
    put(root.children[2], { y: (1 - sp) * 16, o: sp });
  }
  const stage = $('#stage');
  /** 元素相对某个容器（默认舞台）的位置；只在 init 时量一次 */
  function rel(e, container = stage) {
    const a = e.getBoundingClientRect();
    const b = container.getBoundingClientRect();
    return { x: a.left - b.left, y: a.top - b.top, w: a.width, h: a.height };
  }
  /** 选区框：蓝色细边 + 四角白色把手 + 外面压暗 */
  function selHTML(cls, dim = 0.5) {
    return `<div class="abs ${cls}" style="box-shadow:0 0 0 4000px rgba(0,0,0,${dim});outline:2px solid #1e90ff;outline-offset:0">${['-5px auto auto -5px', '-5px -5px auto auto', 'auto -5px -5px auto', 'auto auto -5px -5px']
      .map((m) => {
        const [tp, rt, bt, lf] = m.split(' ');
        return `<i style="position:absolute;top:${tp};right:${rt};bottom:${bt};left:${lf};width:10px;height:10px;border-radius:50%;background:#fff;border:2px solid #1e90ff"></i>`;
      })
      .join('')}</div>`;
  }
  function placeSel(e, r, o = 1) {
    e.style.left = `${r.x.toFixed(1)}px`;
    e.style.top = `${r.y.toFixed(1)}px`;
    e.style.width = `${Math.max(0, r.w).toFixed(1)}px`;
    e.style.height = `${Math.max(0, r.h).toFixed(1)}px`;
    e.style.opacity = o;
  }
  const lerpRect = (a, b, p) => ({ x: lerp(a.x, b.x, p), y: lerp(a.y, b.y, p), w: lerp(a.w, b.w, p), h: lerp(a.h, b.h, p) });
  const pad = (r, p) => ({ x: r.x - p, y: r.y - p, w: r.w + 2 * p, h: r.h + 2 * p });

  // ═════════════════════════ S4 截图（20–30） ═════════════════════════
  const SCR = { x: 740, y: 140, w: 1080, h: 800 };
  const mosaicR = rng(99);
  const S4 = scene(
    's4', 19.98, 30.05,
    `${hdrHTML('s4-hdr', '01', T.s4)}
     ${keysHTML('s4-keys', ['F1'], T.s4hint, 120, 640)}
     <div class="abs s4-screen" style="left:${SCR.x}px;top:${SCR.y}px;width:${SCR.w}px;height:${SCR.h}px;border-radius:26px;overflow:hidden;
        background:radial-gradient(ellipse 70% 60% at 80% 0%, #2c6b5c, transparent 70%),radial-gradient(ellipse 60% 70% at 0% 100%, #2b4566, transparent 70%),linear-gradient(160deg,#1a2b2f,#121820);
        box-shadow:0 50px 120px rgba(0,0,0,.6), 0 0 0 1px rgba(255,255,255,.08)">
       <div class="win s4-browser" style="left:50px;top:40px;width:980px;height:720px;background:#15181c">
         <div class="win-bar" style="height:48px"><span class="d"></span><span class="d"></span><span class="d"></span><div style="margin-left:14px;flex:1;height:30px;border-radius:8px;background:rgba(255,255,255,.06);display:flex;align-items:center;padding:0 14px;font:500 15px var(--sans);color:var(--ink3)">${esc(T.s4url)}</div></div>
         <div style="position:absolute;left:0;top:48px;right:0;bottom:0;padding:40px 64px">
           <div style="font:600 16px var(--sans);color:#5fd4a8">${esc(T.s4tag)}</div>
           <div class="s4-h1" style="margin-top:12px;font:800 36px/1.3 var(--sans)">${esc(T.s4h1)}</div>
           <p class="s4-p1" style="margin:20px 0 0;font:500 22px/1.85 var(--sans);color:rgba(235,240,245,.8)">${esc(T.s4p1[0])}<span class="s4-hl">${esc(T.s4p1[1])}</span></p>
           <p class="s4-p2" style="margin:6px 0 0;font:500 22px/1.85 var(--sans);color:rgba(235,240,245,.8)">${esc(T.s4p2)}</p>
           <div style="display:flex;gap:30px;margin-top:28px">
             <div class="s4-img" style="position:relative;flex:none;width:370px;height:196px;border-radius:14px;overflow:hidden;background:linear-gradient(180deg,#ffd29a 0%,#f49a7c 48%,#6d5a96 100%)">
               <div style="position:absolute;left:236px;top:46px;width:56px;height:56px;border-radius:50%;background:#fff3d6"></div>
               <div style="position:absolute;inset:0;background:#4b3f78;clip-path:polygon(0 100%,0 62%,22% 38%,40% 60%,58% 30%,80% 58%,100% 44%,100% 100%)"></div>
               <div style="position:absolute;inset:0;background:#2e2752;clip-path:polygon(0 100%,0 80%,30% 60%,52% 78%,74% 56%,100% 76%,100% 100%)"></div>
             </div>
             <div style="flex:1;padding-top:4px"><div class="ln" style="width:94%"></div><div class="ln" style="width:82%"></div><div class="ln" style="width:88%"></div><div class="ln" style="width:60%"></div>
               <div class="s4-mail" style="display:inline-block;margin-top:20px;font:500 19px var(--sans);color:var(--ink2)">${esc(T.s4mail)}</div></div>
           </div>
         </div>
       </div>
       <canvas class="abs s4-mosaic" width="${SCR.w}" height="${SCR.h}"></canvas>
       <svg class="abs s4-ann" width="${SCR.w}" height="${SCR.h}"><rect class="s4-ann-rect" fill="none" stroke="#ff3b30" stroke-width="4.5" rx="4"/><path class="s4-ann-arrow" fill="#ff3b30" stroke="none"/></svg>
       ${selHTML('s4-sel', 0.5)}
       <div class="abs s4-dimlab glass" style="padding:6px 12px;border-radius:9px;font:600 17px var(--mono);white-space:nowrap"></div>
       ${toolbarHTML('s4-tb')}
       <div class="abs s4-flash" style="background:#fff;opacity:0"></div>
       <div class="abs s4-toast toast glass"><span class="ok">${icon('check', 18, '#06130d', 3)}</span>${esc(T.s4toast)}</div>
     </div>
     <div class="abs s4-tiles" style="left:${SCR.x}px;top:${SCR.y}px;width:${SCR.w}px;height:${SCR.h}px">
       ${T.s4tiles.map(([name, key, desc], i) => `<div class="abs s4-tile glass" style="left:${(i % 2) * 550}px;top:${Math.floor(i / 2) * 410}px;width:530px;height:390px;border-radius:26px;overflow:hidden">
          <div class="s4-tv" style="position:absolute;left:24px;top:24px;right:24px;height:240px;border-radius:16px;overflow:hidden;background:linear-gradient(160deg,#1d2a2e,#141a20)">${tileBody(i)}</div>
          <div style="position:absolute;left:30px;bottom:30px;right:30px"><div style="display:flex;align-items:center;gap:14px;font:800 32px var(--sans)">${esc(name)}<span style="font:700 17px var(--mono);color:var(--mint);padding:4px 10px;border-radius:8px;background:rgba(45,245,176,.12)">${esc(key)}</span></div><div style="margin-top:8px;font:500 20px var(--sans);color:var(--ink2)">${esc(desc)}</div></div>
        </div>`).join('')}
     </div>`,
    (t, sc) => {
      animHdr(sc, 's4-hdr', t, 20.15, 29.55);
      animKeys(sc, 's4-keys', t, 20.25, 20.6, 29.5);
      const L = S4.L;
      const scr = sc.q('.s4-screen');
      const sin = P(t, 20.05, 20.6, E.outCubic);
      const sout = P(t, 25.85, 26.3, E.inCubic);
      put(scr, { y: (1 - sin) * 60, s: 1 - 0.06 * sout, o: sin * (1 - sout), blur: sout * 8 });

      // 遮罩里的"洞"：先跟着悬停的区域走（自动识别），按下后变成拖出来的选区
      const sel = sc.q('.s4-sel');
      const maskOn = P(t, 20.6, 20.72);
      let hole = null;
      const cur = { x: 540, y: 420 };
      let curType = 'cross';
      let curPress = 0;
      const tDown = 21.45;
      const tUp = 22.3;
      if (t < 20.6) {
        sel.style.opacity = 0;
      } else {
        if (t < 20.9) {
          hole = { x: 540, y: 400, w: 0, h: 0 };
          const p = P(t, 20.62, 20.9, E.inOutCubic);
          cur.x = lerp(560, L.img.x + L.img.w / 2, p);
          cur.y = lerp(470, L.img.y + L.img.h / 2, p);
        } else if (t < 21.15) {
          hole = L.img;
          cur.x = L.img.x + L.img.w / 2 + (t - 20.9) * 60;
          cur.y = L.img.y + L.img.h / 2 - (t - 20.9) * 120;
        } else if (t < tDown) {
          const p = P(t, 21.15, 21.3, E.outCubic);
          hole = lerpRect(L.img, L.browser, p);
          const q = P(t, 21.15, tDown, E.inOutCubic);
          cur.x = lerp(L.img.x + L.img.w / 2 + 15, L.selA.x, q);
          cur.y = lerp(L.img.y + L.img.h / 2 - 30, L.selA.y, q);
        } else {
          const p = P(t, tDown + 0.04, tUp, E.inOutCubic);
          const r = lerpRect({ x: L.selA.x, y: L.selA.y, w: 0, h: 0 }, L.sel, p);
          hole = r;
          if (t < tUp + 0.05) {
            cur.x = r.x + r.w;
            cur.y = r.y + r.h;
            curPress = t < tUp ? 1 : 0;
          }
        }
        const lift = P(t, 25.3, 25.6);
        placeSel(sel, hole, maskOn * (1 - lift));
        sel.style.outlineColor = t < tDown ? 'rgba(30,144,255,.95)' : '#1e90ff';
        for (const h of sel.children) h.style.opacity = t > tDown ? 1 : 0;
        const lab = sc.q('.s4-dimlab');
        if (t > tDown) {
          lab.textContent = `${Math.round(hole.w)} × ${Math.round(hole.h)}`;
          lab.style.left = `${hole.x}px`;
          lab.style.top = `${hole.y - 42}px`;
          lab.style.opacity = 1 - lift;
        } else lab.style.opacity = 0;
      }

      // 工具条 + 标注
      const tb = sc.q('.s4-tb');
      const tbp = P(t, tUp + 0.02, tUp + 0.32, E.outBack);
      put(tb, { y: (1 - tbp) * 14, s: 0.94 + 0.06 * tbp, o: P(t, tUp, tUp + 0.12) * (1 - P(t, 25.3, 25.5)) });
      const tool = t > 24.75 ? 'check' : t > 24.1 ? 'mosaic' : t > 23.45 ? 'arrow' : t > 22.8 ? 'rect' : null;
      for (const b of sc.qa('.s4-tb .tb-btn')) {
        if (b.dataset.k === 'check') continue;
        b.style.background = b.dataset.k === tool ? 'rgba(45,245,176,.22)' : 'transparent';
        b.style.color = b.dataset.k === tool ? '#2df5b0' : 'rgba(240,244,247,.86)';
      }
      const btn = (k) => L.tb[k];
      const moveTo = (from, to, a, b) => {
        const p = P(t, a, b, E.inOutCubic);
        return { x: lerp(from.x, to.x, p), y: lerp(from.y, to.y, p) };
      };
      const hl = L.hl;
      const rA = { x: hl.x - 10, y: hl.y - 6 };
      const rB = { x: hl.x + hl.w + 10, y: hl.y + hl.h + 6 };
      const arA = { x: rB.x + 150, y: rB.y + 110 };
      const arB = { x: rB.x + 14, y: rB.y + 12 };
      const mail = L.mail;
      const mA = { x: mail.x - 6, y: mail.y + mail.h / 2 };
      const mB = { x: mail.x + mail.w + 8, y: mail.y + mail.h / 2 };
      if (t >= tUp + 0.05) {
        curType = 'arrow';
        let c;
        if (t < 22.8) c = moveTo({ x: L.sel.x + L.sel.w, y: L.sel.y + L.sel.h }, btn('rect'), tUp + 0.1, 22.75);
        else if (t < 22.85) c = btn('rect');
        else if (t < 23.4) {
          curType = 'cross';
          c = t < 22.95 ? moveTo(btn('rect'), rA, 22.8, 22.95) : moveTo(rA, rB, 22.98, 23.35);
          curPress = t > 22.96 && t < 23.36 ? 1 : 0;
        } else if (t < 23.5) c = moveTo(rB, btn('arrow'), 23.38, 23.48);
        else if (t < 24.05) {
          curType = 'cross';
          c = t < 23.6 ? moveTo(btn('arrow'), arA, 23.5, 23.6) : moveTo(arA, arB, 23.62, 24.0);
          curPress = t > 23.61 && t < 24.0 ? 1 : 0;
        } else if (t < 24.15) c = moveTo(arB, btn('mosaic'), 24.02, 24.13);
        else if (t < 24.75) {
          curType = 'cross';
          c = t < 24.25 ? moveTo(btn('mosaic'), mA, 24.15, 24.25) : moveTo(mA, mB, 24.27, 24.7);
          curPress = t > 24.26 && t < 24.7 ? 1 : 0;
        } else c = moveTo(mB, btn('check'), 24.72, 24.95);
        cur.x = c.x;
        cur.y = c.y;
        curPress = Math.max(curPress, press(t, 22.8, 0.05), press(t, 23.45, 0.05), press(t, 24.12, 0.05), press(t, 25.0, 0.06));
      }
      // 矩形
      const rect = sc.q('.s4-ann-rect');
      const rp = P(t, 22.98, 23.35, E.inOutCubic);
      if (t > 22.97) {
        rect.setAttribute('x', rA.x);
        rect.setAttribute('y', rA.y);
        rect.setAttribute('width', Math.max(0.1, (rB.x - rA.x) * rp));
        rect.setAttribute('height', Math.max(0.1, (rB.y - rA.y) * rp));
        rect.style.opacity = 1;
      } else rect.style.opacity = 0;
      // 箭头：粗线 + 三角头
      const ar = sc.q('.s4-ann-arrow');
      const ap = P(t, 23.62, 24.0, E.inOutCubic);
      if (ap > 0) {
        const ex = lerp(arA.x, arB.x, ap);
        const ey = lerp(arA.y, arB.y, ap);
        const ang = Math.atan2(ey - arA.y, ex - arA.x);
        const len = Math.hypot(ex - arA.x, ey - arA.y);
        const hw = 14;
        const hl2 = Math.min(30, len);
        const bx = ex - Math.cos(ang) * hl2;
        const by = ey - Math.sin(ang) * hl2;
        const nx = -Math.sin(ang);
        const ny = Math.cos(ang);
        const sw = 3;
        ar.setAttribute('d', `M${arA.x + nx * 1} ${arA.y + ny * 1} L${bx + nx * sw} ${by + ny * sw} L${bx + nx * hw} ${by + ny * hw} L${ex} ${ey} L${bx - nx * hw} ${by - ny * hw} L${bx - nx * sw} ${by - ny * sw} L${arA.x - nx * 1} ${arA.y - ny * 1}Z`);
        ar.style.opacity = 1;
      } else ar.style.opacity = 0;
      // 马赛克：笔刷扫过的地方变成色块
      const mc = sc.q('.s4-mosaic');
      const g = mc.getContext('2d');
      g.clearRect(0, 0, SCR.w, SCR.h);
      const mp = P(t, 24.27, 24.7, E.inOutCubic);
      if (mp > 0) {
        const mx = lerp(mA.x, mB.x, mp);
        for (const cell of S4.mosaic) if (cell.x < mx) {
          g.fillStyle = cell.c;
          g.fillRect(cell.x, cell.y, 12, 12);
        }
      }
      // 完成：闪一下白，提示已复制
      const fl = sc.q('.s4-flash');
      placeSel(fl, L.sel, t > 25 ? 0.7 * Math.exp(-(t - 25) * 7) : 0);
      const toast = sc.q('.s4-toast');
      const tp = P(t, 25.1, 25.45, E.outBack);
      toast.style.left = `${SCR.w / 2 - 150}px`;
      toast.style.top = `${SCR.h - 110}px`;
      put(toast, { y: (1 - tp) * 30, s: 0.9 + 0.1 * tp, o: P(t, 25.1, 25.25) * (1 - P(t, 25.75, 25.95)) });

      if (t > 20.55 && t < 25.3) setCursor(curType, SCR.x + cur.x, SCR.y + cur.y, { press: curPress, o: 1 - P(t, 25.1, 25.3) });

      // 四个小功能卡片
      sc.qa('.s4-tile').forEach((tile, i) => {
        const a = 26.0 + i * 0.5;
        const p = P(t, a, a + 0.5, E.outBack);
        const out = P(t, 29.45 + i * 0.05, 29.85 + i * 0.05, E.inCubic);
        put(tile, { y: (1 - p) * 70 - out * 40, s: (0.9 + 0.1 * p) * (1 - 0.05 * out), o: P(t, a, a + 0.15) * (1 - out) });
        tileAnim(i, t - a, tile);
      });
    },
  );
  function tileBody(i) {
    if (i === 0)
      return `<div class="t0-view" style="position:absolute;left:30px;top:22px;width:250px;height:196px;border-radius:10px;overflow:hidden;background:#f2f4f6;outline:2px solid #1e90ff"><div class="t0-page" style="padding:14px 18px">${Array.from({ length: 22 }, (_, k) => `<div style="height:${k % 6 === 0 ? 16 : 9}px;margin:${k % 6 === 0 ? 14 : 8}px 0;border-radius:4px;width:${[88, 72, 95, 64, 80, 90][k % 6]}%;background:${k % 6 === 0 ? '#3d4a57' : '#c9d0d8'}"></div>`).join('')}</div></div>
        <div style="position:absolute;left:300px;top:22px;width:90px;height:196px;display:flex;align-items:center;justify-content:center;color:var(--mint)">${icon('scroll', 34, '#2df5b0')}</div>
        <div class="t0-strip" style="position:absolute;left:400px;top:22px;width:56px;height:30px;border-radius:6px;background:repeating-linear-gradient(180deg,#e9edf1 0 10px,#c9d0d8 10px 14px);box-shadow:0 0 0 2px #2df5b0"></div>`;
    if (i === 1)
      return `<div style="position:absolute;left:60px;top:30px;width:360px;height:180px;border:2.5px dashed #ff453a;border-radius:6px"><div class="t1-ball" style="position:absolute;left:30px;top:120px;width:36px;height:36px;border-radius:50%;background:linear-gradient(135deg,#e9ff5a,#2df5b0)"></div></div>
        <div style="position:absolute;left:66px;top:8px;display:flex;align-items:center;gap:8px;padding:3px 10px;border-radius:8px;background:#ff453a;font:700 15px var(--mono)"><i class="t1-dot" style="width:9px;height:9px;border-radius:50%;background:#fff"></i>REC <span class="t1-time">00:00</span></div>`;
    if (i === 2)
      return `<div style="position:absolute;left:30px;top:26px;width:300px;height:190px;border-radius:10px;background:#20262c;padding:16px 18px"><div class="ln" style="width:70%"></div><div class="ln"></div><div class="ln" style="width:85%"></div><div class="ln" style="width:60%"></div><div class="ln" style="width:78%"></div></div>
        <div class="t2-pin" style="position:absolute;left:220px;top:40px;width:220px;height:150px;border-radius:10px;background:linear-gradient(180deg,#ffd29a,#f49a7c 50%,#6d5a96);box-shadow:0 20px 40px rgba(0,0,0,.55),0 0 0 2px rgba(45,245,176,.8)"><div style="position:absolute;inset:0;background:#4b3f78;clip-path:polygon(0 100%,0 62%,22% 38%,40% 60%,58% 30%,80% 58%,100% 44%,100% 100%);border-radius:10px"></div><div style="position:absolute;right:-12px;top:-14px;width:34px;height:34px;border-radius:50%;background:var(--mint);display:grid;place-items:center">${icon('pin', 18, '#06130d', 2.4)}</div></div>`;
    return `<div style="position:absolute;left:40px;top:24px;width:240px;height:40px;border-radius:8px;background:#2a3036;display:flex;align-items:center;padding:0 14px;font:600 17px var(--sans);color:var(--ink2);gap:8px">${esc(T.s4menu[0])} ${icon('down', 16)}</div>
      <div style="position:absolute;left:40px;top:70px;width:240px;border-radius:10px;background:#2f363d;box-shadow:0 14px 30px rgba(0,0,0,.5);padding:6px">${T.s4menu[1].map((s, k) => `<div style="padding:9px 12px;border-radius:7px;font:500 16px var(--sans);${k === 2 ? 'background:#0a84ff;color:#fff' : 'color:var(--ink2)'}">${s}</div>`).join('')}</div>
      <div class="t3-sel" style="position:absolute;left:28px;top:14px;width:268px;height:224px;outline:2px solid #1e90ff;box-shadow:0 0 0 2000px rgba(0,0,0,.35)"></div>
      <div class="t3-flash" style="position:absolute;inset:0;background:#fff;opacity:0"></div>
      <div style="position:absolute;right:40px;top:80px;width:84px;height:84px;border-radius:50%;background:rgba(45,245,176,.14);display:grid;place-items:center">${icon('zap', 40, '#2df5b0')}</div>`;
  }
  function tileAnim(i, lt, tile) {
    if (lt < -0.1) return;
    if (i === 0) {
      const p = clamp(lt / 2.6);
      tile.querySelector('.t0-page').style.transform = `translateY(${(-E.inOutSine(p) * 260).toFixed(1)}px)`;
      tile.querySelector('.t0-strip').style.height = `${(30 + 166 * E.inOutSine(p)).toFixed(1)}px`;
    } else if (i === 1) {
      const b = tile.querySelector('.t1-ball');
      const x = 20 + ((lt * 160) % 300);
      const y = 120 - Math.abs(Math.sin(lt * 5)) * 100;
      b.style.transform = `translate(${x.toFixed(1)}px,${(y - 120).toFixed(1)}px)`;
      tile.querySelector('.t1-time').textContent = `00:0${Math.min(9, Math.floor(Math.max(0, lt) * 2))}`;
      tile.querySelector('.t1-dot').style.opacity = Math.floor(lt * 3) % 2 ? 0.25 : 1;
    } else if (i === 2) {
      const pin = tile.querySelector('.t2-pin');
      const lift = P(lt, 0, 0.5, E.outBack);
      pin.style.transform = `translateY(${(Math.sin(lt * 2.4) * 6 - lift * 10).toFixed(1)}px) rotate(${(-4 * lift).toFixed(2)}deg) scale(${(0.9 + 0.1 * lift).toFixed(3)})`;
    } else {
      tile.querySelector('.t3-flash').style.opacity = lt > 0.25 ? (0.8 * Math.exp(-(lt - 0.25) * 6)).toFixed(3) : 0;
      tile.querySelector('.t3-sel').style.opacity = lt > 0.25 ? 1 : 0;
    }
  }
  afterLayout.push(() => {
    const scr = S4.q('.s4-screen');
    const r = (s) => rel(S4.q(s), scr);
    const L = (S4.L = {});
    L.img = pad(r('.s4-img'), 0);
    L.browser = r('.s4-browser');
    L.hl = r('.s4-hl');
    L.mail = r('.s4-mail');
    const h1 = r('.s4-h1');
    const p2 = r('.s4-p2');
    L.sel = { x: h1.x - 22, y: h1.y - 18, w: Math.max(h1.w, p2.w) + 44, h: p2.y + p2.h - h1.y + 36 };
    // 选区要把邮箱那行也框进来，好演示马赛克
    L.sel.h = L.mail.y + L.mail.h + 18 - L.sel.y;
    L.selA = { x: L.sel.x, y: L.sel.y };
    // 工具条贴在选区下方右对齐
    const tb = S4.q('.s4-tb');
    const tbw = tb.offsetWidth;
    tb.style.left = `${L.sel.x + L.sel.w - tbw}px`;
    tb.style.top = `${L.sel.y + L.sel.h + 12}px`;
    L.tb = {};
    for (const b of S4.qa('.s4-tb .tb-btn')) {
      const br = rel(b, scr);
      L.tb[b.dataset.k] = { x: br.x + br.w / 2 + 6, y: br.y + br.h / 2 + 8 };
    }
    // 马赛克色块
    S4.mosaic = [];
    for (let y = L.mail.y - 4; y < L.mail.y + L.mail.h + 2; y += 12) {
      for (let x = L.mail.x - 6; x < L.mail.x + L.mail.w + 8; x += 12) {
        const v = 60 + mosaicR() * 70;
        S4.mosaic.push({ x, y, c: `rgb(${v | 0},${(v + 6) | 0},${(v + 12) | 0})` });
      }
    }
  });
  cue(20.6, 'key');
  cue(20.95, 'tick');
  cue(21.2, 'tick');
  cue(21.45, 'click');
  cue(22.3, 'click', { up: 1 });
  cue(22.33, 'pop', { soft: 1 });
  cue(22.8, 'click');
  cue(22.98, 'draw', { dur: 0.37 });
  cue(23.45, 'click');
  cue(23.62, 'draw', { dur: 0.38 });
  cue(24.12, 'click');
  cue(24.27, 'mosaic', { dur: 0.43 });
  cue(25.0, 'shutter');
  cue(25.1, 'success');
  [26.0, 26.5, 27.0, 27.5].forEach((a, i) => cue(a, 'card', { n: i }));
  cue(27.75, 'shutter', { soft: 1 });

  // ═════════════════════════ S5 识字 + S6 原位翻译（30–42）：开场那张图又回来了 ═════════════════════════
  const C5 = { x: 340, y: 360 };
  const S5 = scene(
    's5', 29.85, 42.4,
    `${hdrHTML('s5-hdr', '02', T.s5, 'style="width:1240px"')}
     ${hdrHTML('s6-hdr', '03', T.s6, 'style="width:1240px"')}
     ${keysHTML('s5-keys', ['F3'], T.s5hint, 1640, 104)}
     ${keysHTML('s6-keys', ['Ctrl', 'T'], T.s6hint, 1428, 104)}
     <div class="abs s5-card" style="left:${C5.x}px;top:${C5.y}px;width:${CARD.w}px;height:${CARD.h}px;border-radius:28px;${CARD_BG}box-shadow:0 40px 120px rgba(0,0,0,.55);border:1px solid rgba(255,255,255,.07)">
       <canvas class="abs s5-cv" width="${CARD.w}" height="${CARD.h}"></canvas>
       <div class="abs s6-label" style="left:0;top:0;border-radius:16px;background:rgba(17,40,44,.9);backdrop-filter:blur(10px);display:grid;place-items:center;font:900 0px var(--sans);color:#f4fffb;white-space:nowrap"><span class="s6-zh"></span></div>
       <div class="abs s6-svc chip" style="left:auto;right:22px;top:20px;font-size:18px;padding:7px 14px;background:rgba(0,0,0,.35)">${esc(T.s6services[0][0])} ${icon('down', 16)}</div>
     </div>
     <div class="abs s5-sel" style="box-shadow:0 0 0 4000px rgba(0,0,0,.5);outline:2px solid #1e90ff"></div>
     <div class="abs s5-beam" style="left:0;top:0;width:180px;height:${CARD.h + 40}px;background:linear-gradient(90deg,transparent,rgba(45,245,176,.0) 30%,rgba(45,245,176,.28) 88%,#eafff6 99%,transparent);filter:blur(1px)"></div>
     <div class="caption s5-yes" style="top:${C5.y + CARD.h + 46}px;font-size:40px;color:var(--mint)">${chars(T.s5yes)}</div>
     <div class="abs s5-chips" style="left:0;right:0;top:${C5.y + CARD.h + 50}px;display:flex;justify-content:center;gap:18px">${[['wifiOff', 0], ['shield', 1], ['copy', 2]].map(([k, i]) => `<span class="chip s5-chip">${icon(k, 19, '#2df5b0')}${esc(T.s5chips[i])}</span>`).join('')}</div>
     <div class="abs s5-toast toast glass"><span class="ok">${icon('check', 18, '#06130d', 3)}</span>${esc(T.s5toast)}</div>
     <div class="abs s6-chips" style="left:0;right:0;top:${C5.y + CARD.h + 50}px;display:flex;justify-content:center;gap:18px">${T.s6chips.map((c) => `<span class="chip s6-chip">${icon('check', 18, '#2df5b0', 2.6)}${esc(c)}</span>`).join('')}</div>`,
    (t, sc) => {
      animHdr(sc, 's5-hdr', t, 30.0, 37.45);
      animHdr(sc, 's6-hdr', t, 37.95, 41.45);
      animKeys(sc, 's5-keys', t, 30.15, 30.5, 37.6);
      animKeys(sc, 's6-keys', t, 38.0, 38.55, 41.7);

      const card = sc.q('.s5-card');
      const cin = P(t, 29.9, 30.5, E.outCubic);
      const cout = P(t, 41.6, 42.2, E.inCubic);
      put(card, { y: (1 - cin) * 50 - cout * 30, s: lerp(0.95, 1, cin) * (1 - 0.06 * cout), o: cin * (1 - cout), blur: cout * 10 });

      // 选区：光标从左上拖到右下，框住整张图
      const sel = sc.q('.s5-sel');
      const A = { x: C5.x - 18, y: C5.y - 18 };
      const B = { x: C5.x + CARD.w + 18, y: C5.y + CARD.h + 18 };
      const sp = P(t, 30.62, 31.0, E.inOutCubic);
      const selOut = P(t, 34.9, 35.3);
      if (t > 30.5 && t < 35.4) {
        placeSel(sel, { x: A.x, y: A.y, w: (B.x - A.x) * sp, h: (B.y - A.y) * sp }, P(t, 30.5, 30.6) * (1 - selOut));
      } else sel.style.opacity = 0;
      if (t > 30.5 && t < 31.1) setCursor('cross', lerp(A.x, B.x, sp), lerp(A.y, B.y, sp), { press: t > 30.6 && t < 31 ? 1 : 0, o: 1 - P(t, 31.0, 31.1) });

      // 扫描线
      const scanA = 31.05;
      const scanB = 32.85;
      const sx = lerp(-120, CARD.w + 260, P(t, scanA, scanB, E.inOutSine));
      const beam = sc.q('.s5-beam');
      put(beam, { x: C5.x + sx - 180, y: C5.y - 20, o: t > scanA && t < scanB + 0.05 ? 1 : 0 });

      // 画布：点阵 → 光条 → 清晰的字
      const cv = sc.q('.s5-cv');
      const g = cv.getContext('2d');
      g.clearRect(0, 0, CARD.w, CARD.h);
      const L = quoteLayout.boxes;
      const scanned = t >= scanA;
      const curX = scanned ? sx : -1e9;
      g.globalAlpha = 1 - P(t, scanA, scanB);
      g.drawImage(quoteGrid, 0, 0);
      g.globalAlpha = 1;
      // 选中高亮（字的下面）
      const hp1 = P(t, 33.1, 33.5, E.inOutSine);
      const hp2 = P(t, 33.5, 33.95, E.inOutSine);
      const hlO = 1 - P(t, 37.3, 37.6);
      if (hp1 > 0 && hlO > 0) {
        g.fillStyle = `rgba(10,132,255,${(0.55 * hlO).toFixed(3)})`;
        const r0 = L[0];
        const r1 = L[1];
        g.fillRect(r0.x - 6, r0.top, (r0.w + 12) * hp1, r0.h);
        if (hp2 > 0) g.fillRect(r1.x - 6, r1.top, (r1.w + 12) * hp2, r1.h);
      }
      // 还没扫到的点
      for (const d of quoteDots) {
        const q = clamp((curX - d.x) / 150);
        if (q >= 1) continue;
        const row = d.y < (L[0].base + L[1].top) / 2 ? L[0] : L[1];
        const e = E.inCubic(q);
        const y = lerp(d.y, row.mid, e);
        const x = d.x + e * 30;
        g.fillStyle = dotColor(d.v, (1 - q) * (1 - 0.5 * e));
        g.beginPath();
        g.arc(x, y, d.rad * (1 - 0.7 * e), 0, Math.PI * 2);
        g.fill();
      }
      // 扫描线后面一小段变成光条（和图标里的线一样）
      if (scanned && t < scanB + 0.4) {
        for (const r of L) {
          const x1 = Math.max(r.x, curX - 330);
          const x2 = Math.min(r.x + r.w, curX - 40);
          if (x2 <= x1) continue;
          const grd = g.createLinearGradient(curX - 330, 0, curX - 40, 0);
          grd.addColorStop(0, 'rgba(233,255,90,0)');
          grd.addColorStop(0.5, 'rgba(233,255,90,.55)');
          grd.addColorStop(1, 'rgba(45,245,176,.95)');
          g.fillStyle = grd;
          const hh = quoteLayout.fs * 0.36;
          g.beginPath();
          g.roundRect(x1, r.mid - hh / 2, x2 - x1, hh, hh / 2);
          g.fill();
        }
      }
      // 清晰的字：从左往右显出来
      if (scanned) {
        const tc = S5.textMask;
        const m = tc.getContext('2d');
        m.clearRect(0, 0, CARD.w, CARD.h);
        m.globalCompositeOperation = 'source-over';
        m.drawImage(quoteLayout.canvas, 0, 0);
        m.globalCompositeOperation = 'destination-in';
        const grd = m.createLinearGradient(curX - 340, 0, curX - 170, 0);
        grd.addColorStop(0, 'rgba(0,0,0,1)');
        grd.addColorStop(1, 'rgba(0,0,0,0)');
        m.fillStyle = grd;
        m.fillRect(0, 0, CARD.w, CARD.h);
        // 原位翻译时原文淡出
        g.globalAlpha = 1 - 0.85 * P(t, 38.9, 39.3);
        g.drawImage(tc, 0, 0);
        g.globalAlpha = 1;
      }

      // 这次，选得中了
      if (t > 32.9 && t < 34.3) {
        const r0 = L[0];
        const r1 = L[1];
        const p0 = { x: C5.x + r0.x - 6, y: C5.y + r0.mid };
        const p1 = { x: C5.x + r1.x + r1.w + 8, y: C5.y + r1.mid };
        let x;
        let y;
        if (t < 33.1) {
          const p = P(t, 32.9, 33.08, E.outCubic);
          x = lerp(p0.x + 140, p0.x, p);
          y = lerp(p0.y + 120, p0.y, p);
        } else if (t < 33.5) {
          x = lerp(p0.x, C5.x + r0.x + r0.w + 6, hp1);
          y = p0.y;
        } else {
          x = lerp(C5.x + r1.x - 6, p1.x, hp2);
          y = p1.y;
        }
        setCursor('ibeam', x, y, { press: t > 33.1 && t < 33.95 ? 1 : 0, o: P(t, 32.9, 33.0) * (1 - P(t, 34.1, 34.3)) });
      }
      revealChars(sc.q('.s5-yes'), t, 33.95, { stagger: 0.06, dur: 0.5, out: 35.2, outDur: 0.3 });
      const toast = sc.q('.s5-toast');
      if (!toast._w) toast._w = toast.offsetWidth;
      toast.style.left = `${960 - toast._w / 2}px`;
      toast.style.top = `${C5.y + CARD.h + 130}px`;
      const tp = P(t, 34.3, 34.65, E.outBack);
      put(toast, { y: (1 - tp) * 24, s: 0.9 + 0.1 * tp, o: P(t, 34.3, 34.45) * (1 - P(t, 37.2, 37.5)) });

      sc.qa('.s5-chip').forEach((c, i) => {
        const a = 35.5 + i * 0.25;
        const p = P(t, a, a + 0.4, E.outBack);
        put(c, { y: (1 - p) * 20, o: P(t, a, a + 0.15) * (1 - P(t, 37.2, 37.5)) });
      });
      // 原位翻译：一道光扫过，译文标签盖在原文的位置，逐字从乱码落定
      const lab = sc.q('.s6-label');
      const blk = S5.block;
      const lp = P(t, 38.85, 39.2, E.outCubic);
      lab.style.left = `${blk.x}px`;
      lab.style.top = `${blk.y}px`;
      lab.style.width = `${blk.w}px`;
      lab.style.height = `${blk.h}px`;
      lab.style.fontSize = `${S5.zhFs}px`;
      put(lab, { s: 0.97 + 0.03 * lp, o: lp });
      const zh = sc.q('.s6-zh');
      const target = [...T.s6zh];
      const pool = '的一是了我不人在他有这个上们来到时大地为子中你说生国年着就那和要她出也得里后自以会家可下而过天去能对小多然于心学么之都好看起发当没成只如事把还用第样道想作种开美总从无情己面最女但现前些所同日手又行意动方期它头经长儿回位分爱老因很给名法间斯知世什两次使身者被高已亲其进此话常与活正感';
      const pr = rng(Math.floor(t * 30) + 5);
      let s = '';
      target.forEach((c, i) => {
        const a = 39.1 + i * 0.07;
        if (t < a) s += '　';
        else if (t < a + 0.22 && c !== '。') s += pool[Math.floor(pr() * pool.length)];
        else s += c;
      });
      if (zh._s !== s) {
        zh.textContent = s;
        zh._s = s;
      }
      sc.q('.s6-svc').style.opacity = P(t, 39.6, 39.9);
      put(sc.q('.s6-svc'), { y: (1 - P(t, 39.6, 39.9, E.outBack)) * -10, o: P(t, 39.6, 39.9) });
      // 扫过的那道光
      if (t > 38.6 && t < 39.2) {
        const bx = lerp(-200, CARD.w + 200, P(t, 38.6, 39.15, E.inOutSine));
        put(beam, { x: C5.x + bx - 180, y: C5.y - 20, o: 0.8 });
      }
      sc.qa('.s6-chip').forEach((c, i) => {
        const a = 40.1 + i * 0.25;
        const p = P(t, a, a + 0.4, E.outBack);
        put(c, { y: (1 - p) * 20, o: P(t, a, a + 0.15) * (1 - P(t, 41.5, 41.8)) });
      });
    },
  );
  afterLayout.push(() => {
    S5.textMask = document.createElement('canvas');
    S5.textMask.width = CARD.w;
    S5.textMask.height = CARD.h;
    const L = quoteLayout.boxes;
    const x = Math.min(L[0].x, L[1].x) - 24;
    const y = L[0].top - 16;
    const w = Math.max(L[0].w, L[1].w) + 48;
    const h = L[1].top + L[1].h - L[0].top + 32;
    S5.block = { x, y, w, h };
    S5.zhFs = Math.min(Math.floor((w - 60) / [...T.s6zh].length), Math.floor(h * 0.62));
  });
  cue(30.5, 'key');
  cue(30.62, 'click');
  cue(31.0, 'click', { up: 1 });
  cue(31.05, 'scan', { dur: 1.8 });
  cue(33.1, 'click');
  cue(34.3, 'success');
  [35.5, 35.75, 36.0].forEach((a) => cue(a, 'tick'));
  cue(38.55, 'key');
  cue(38.6, 'scan', { dur: 0.55, soft: 1 });
  cue(39.1, 'scramble', { n: [...T.s6zh].length, gap: 0.07 });
  [40.1, 40.35, 40.6].forEach((a) => cue(a, 'tick'));

  // ═════════════════════════ S6b 划词翻译 + 问 AI（42–48） ═════════════════════════
  const ED = { x: 120, y: 420, w: 860, h: 470 };
  const POP = { x: 1150, y: 46, w: 620, h: 984 };
  const S6b = scene(
    's6b', 41.9, 48.1,
    `${hdrHTML('s6b-hdr', '03', T.s6b, 'style="width:980px"')}
     ${keysHTML('s6b-keys', ['Ctrl', 'Alt', 'T'], T.s6bhint, 120, 920)}
     <div class="win s6b-ed" style="left:${ED.x}px;top:${ED.y}px;width:${ED.w}px;height:${ED.h}px;background:#191c20">
       <div class="win-bar" style="height:46px"><span class="d"></span><span class="d"></span><span class="d"></span>${esc(T.s6doc)}</div>
       <div style="padding:34px 46px;font:500 24px/1.9 var(--sans);color:rgba(235,240,245,.75)">
         <div style="font:800 32px/1.4 var(--sans);color:#fff">${esc(T.s6docLines[0])}</div>
         <div style="margin-top:12px">${esc(T.s6docLines[1])}</div>
         <div style="margin-top:14px;font:600 34px/1.5 var(--sans);color:#fff;position:relative;display:inline-block"><span class="s6b-hl abs" style="left:-4px;top:4px;height:46px;background:rgba(10,132,255,.5);border-radius:4px"></span><span class="s6b-q" style="position:relative">${esc(QUOTE.join(' '))}</span></div>
         <div class="ln" style="width:86%;margin-top:30px;height:14px"></div><div class="ln" style="width:62%;height:14px"></div>
       </div>
       <div class="abs s6b-fab" style="width:54px;height:54px;border-radius:50%;background:#20242a;border:1px solid rgba(255,255,255,.14);box-shadow:0 8px 24px rgba(0,0,0,.5);display:grid;place-items:center">${logoSVG('s6b-mini', 34, { base: false, full: true })}</div>
     </div>
     <div class="abs s6b-pop" style="left:${POP.x}px;top:${POP.y}px;width:${POP.w}px;height:${POP.h}px;border-radius:34px;padding:26px 24px;background:rgba(36,48,48,.82);backdrop-filter:blur(30px) saturate(150%);border:1px solid rgba(255,255,255,.1);box-shadow:0 40px 100px rgba(0,0,0,.6)">
       <div style="display:flex;align-items:center;gap:14px;font:600 22px var(--sans);color:var(--ink)">
         <span class="chip" style="font-size:21px;color:var(--ink);background:rgba(255,255,255,.07)">${esc(T.s6from)} ${icon('down', 16)}</span>${icon('swap', 24, 'rgba(240,244,247,.8)')}
         <span class="chip" style="font-size:21px;color:var(--ink);background:rgba(255,255,255,.07)">${esc(T.s6to)} ${icon('down', 16)}</span>
         <span style="flex:1"></span>${icon('pin', 24, 'rgba(240,244,247,.7)')}${icon('sparkles', 24, 'rgba(240,244,247,.7)')}${icon('x', 24, 'rgba(240,244,247,.7)')}
       </div>
       ${T.s6services.map(([name, ch, bgc, res], i) => `<div class="s6b-svc" style="margin-top:${i ? 14 : 22}px;padding:18px 22px 20px;border-radius:20px;background:rgba(255,255,255,.07);border:1px solid rgba(255,255,255,.06)">
          <div style="display:flex;align-items:center;gap:12px;font:600 20px var(--sans);color:var(--ink3)">${icon('down', 18)}<span style="width:30px;height:30px;border-radius:8px;display:grid;place-items:center;font:800 18px var(--sans);background:${bgc};color:${ch === 'G' ? '#4285f4' : '#fff'}">${ch}</span>${esc(name)}${i === 0 ? `<span style="font:600 16px var(--sans);color:#5aa9ff;background:rgba(90,169,255,.16);padding:3px 9px;border-radius:7px">${esc(T.s6default)}</span>` : ''}<span style="flex:1"></span>${icon('copy', 22, 'rgba(240,244,247,.45)')}</div>
          <div class="s6b-res" style="position:relative;margin-top:12px;height:44px;font:500 31px/44px var(--sans);color:#fff"><span class="s6b-txt">${esc(res)}</span><span class="s6b-shim abs" style="left:0;top:10px;width:70%;height:24px;border-radius:8px;background:linear-gradient(90deg,rgba(255,255,255,.06),rgba(255,255,255,.18),rgba(255,255,255,.06));background-size:200% 100%"></span></div>
        </div>`).join('')}
       <div class="s6b-ai" style="margin-top:14px;padding:18px 22px;border-radius:20px;background:linear-gradient(135deg,rgba(45,245,176,.12),rgba(90,169,255,.1));border:1px solid rgba(45,245,176,.22);font:500 22px/1.6 var(--sans);color:#eafff6;min-height:120px">
         <div style="display:flex;gap:10px;align-items:center;font:700 19px var(--sans);color:var(--mint)">${icon('sparkles', 20, '#2df5b0')}AI</div><div class="s6b-ans" style="margin-top:6px"></div></div>
       <div style="position:absolute;left:24px;right:24px;bottom:24px;height:66px;border-radius:33px;background:rgba(255,255,255,.07);border:1px solid rgba(255,255,255,.1);display:flex;align-items:center;gap:14px;padding:0 10px 0 22px;font:500 23px var(--sans)">
         ${icon('sparkles', 26, '#5aa9ff')}<span class="s6b-in" style="flex:1;color:var(--ink3)"></span>
         <span class="s6b-send" style="width:48px;height:48px;border-radius:50%;background:#2c5aa0;display:grid;place-items:center">${icon('up', 24, '#fff', 2.4)}</span></div>
     </div>`,
    (t, sc) => {
      animHdr(sc, 's6b-hdr', t, 42.0, 47.55);
      animKeys(sc, 's6b-keys', t, 42.3, 43.45, 47.5);
      const ed = sc.q('.s6b-ed');
      const ein = P(t, 41.95, 42.45, E.outCubic);
      const eout = P(t, 47.6, 48.05, E.inCubic);
      put(ed, { x: (1 - ein) * -60, y: eout * 30, o: ein * (1 - eout) });
      // 拖选那句话
      const hl = sc.q('.s6b-hl');
      const q = S6b.L.q;
      const hp = P(t, 42.55, 43.1, E.inOutSine);
      hl.style.width = `${(q.w + 8) * hp}px`;
      hl.style.opacity = 1 - P(t, 47.4, 47.7);
      const qx = ED.x + q.x;
      const qy = ED.y + q.y + q.h / 2;
      if (t > 42.35 && t < 43.2) {
        const p0 = P(t, 42.35, 42.55, E.outCubic);
        const x = t < 42.55 ? lerp(qx + 100, qx, p0) : lerp(qx, qx + q.w, hp);
        setCursor('ibeam', x, qy, { press: t > 42.55 && t < 43.1 ? 1 : 0, o: P(t, 42.35, 42.45) });
      }
      // 悬浮按钮
      const fab = sc.q('.s6b-fab');
      const fp = P(t, 43.15, 43.45, E.outBack);
      fab.style.left = `${q.x + q.w + 10}px`;
      fab.style.top = `${q.y + q.h + 6}px`;
      put(fab, { s: fp * (1 - 0.1 * press(t, 43.5, 0.05)), o: P(t, 43.15, 43.25) * (1 - P(t, 43.7, 43.9)) });
      if (t >= 43.2 && t < 44.2) {
        const fx = ED.x + q.x + q.w + 37;
        const fy = ED.y + q.y + q.h + 33;
        const p = P(t, 43.2, 43.45, E.inOutCubic);
        setCursor('arrow', lerp(ED.x + q.x + q.w, fx, p), lerp(qy, fy, p), { press: press(t, 43.5, 0.05), o: 1 - P(t, 43.9, 44.1) });
      }
      // 弹出翻译面板
      const pop = sc.q('.s6b-pop');
      const pp = P(t, 43.55, 44.05, E.outBackSoft);
      put(pop, { x: (1 - pp) * 40, s: 0.94 + 0.06 * pp, o: P(t, 43.55, 43.7) * (1 - eout), blur: (1 - P(t, 43.55, 43.9)) * 10 });
      sc.qa('.s6b-svc').forEach((c, i) => {
        const a = 43.8 + i * 0.25;
        const p = P(t, a, a + 0.35, E.outCubic);
        put(c, { y: (1 - p) * 20, o: p });
        const done = a + 0.3;
        const txt = c.querySelector('.s6b-txt');
        const shim = c.querySelector('.s6b-shim');
        txt.style.opacity = P(t, done, done + 0.15);
        txt.style.filter = t < done + 0.15 ? `blur(${((1 - P(t, done, done + 0.15)) * 6).toFixed(2)}px)` : 'none';
        shim.style.opacity = 1 - P(t, done - 0.05, done + 0.05);
        shim.style.backgroundPosition = `${(-(t * 300) % 200).toFixed(0)}% 0`;
      });
      // 问 AI：打字 → 回车 → 回答一个字一个字流出来
      const inEl = sc.q('.s6b-in');
      const ask = [...T.s6ask];
      const nTyped = Math.floor(clamp((t - 45.0) / 0.1, 0, ask.length));
      const sent = t >= 45.9;
      const caret = Math.floor(t * 3) % 2 && t > 44.7 && !sent ? '|' : '';
      const inTxt = sent ? T.s6askPh : nTyped ? ask.slice(0, nTyped).join('') + caret : t > 44.7 ? caret : T.s6askPh;
      if (inEl._s !== inTxt) {
        inEl.textContent = inTxt;
        inEl._s = inTxt;
        inEl.style.color = !sent && (nTyped || t > 44.7) ? 'var(--ink)' : 'var(--ink3)';
      }
      sc.q('.s6b-send').style.background = nTyped && !sent ? '#0a84ff' : '#2c5aa0';
      const ai = sc.q('.s6b-ai');
      const aip = P(t, 45.95, 46.3, E.outCubic);
      put(ai, { y: (1 - aip) * 20, o: aip });
      const ans = [...T.s6answer];
      const nAns = Math.floor(clamp((t - 46.15) / 0.045, 0, ans.length));
      const ansTxt = `<span style="color:var(--ink2)">${esc(T.s6ask)}</span><br>${esc(ans.slice(0, nAns).join(''))}${nAns < ans.length && t > 46.1 ? '<span style="display:inline-block;width:12px;height:24px;margin-left:3px;vertical-align:-3px;background:#2df5b0;border-radius:2px"></span>' : ''}`;
      const ansEl = sc.q('.s6b-ans');
      if (ansEl._s !== ansTxt) {
        ansEl.innerHTML = ansTxt;
        ansEl._s = ansTxt;
      }
    },
  );
  afterLayout.push(() => {
    S6b.L = { q: rel(S6b.q('.s6b-q'), S6b.q('.s6b-ed')) };
    S6b.qa('.s6b-mini-dot').forEach(() => {});
  });
  cue(42.55, 'click');
  cue(43.15, 'pop', { soft: 1 });
  cue(43.5, 'click');
  cue(43.55, 'whoosh', { dur: 0.4, soft: 1 });
  [43.8, 44.05, 44.3, 44.55].forEach((a, i) => cue(a + 0.3, 'blip', { n: i }));
  [...T.s6ask].forEach((_, i) => cue(45.0 + i * 0.1, 'type'));
  cue(45.9, 'key', { soft: 1 });
  cue(46.15, 'stream', { dur: [...T.s6answer].length * 0.045 });

  // ═════════════════════════ S7 剪贴板（48–58） ═════════════════════════
  const PNL = { x: 60, y: 596, w: 1800, h: 404 };
  const SLOT = (i) => 30 + i * 322;
  const LOGIN = { x: 1080, y: 170, w: 700, h: 380 };
  const appIcon = (kind) => {
    const box = (bg, inner) => `<div style="position:absolute;right:0;top:0;width:66px;height:66px;border-bottom-left-radius:18px;background:${bg};display:grid;place-items:center">${inner}</div>`;
    if (kind === 'otp') return box('linear-gradient(160deg,#5fb4ff,#2f74e8)', icon('mail', 34, '#fff', 2));
    if (kind === 'text' || kind === 'image') return box('#16191d', logoSVG('s7-ico-' + kind, 44, { base: false, full: true }).replace('class="abs ', 'class="'));
    if (kind === 'color') return box('#1c1f23', `<div style="display:grid;grid-template-columns:repeat(2,16px);gap:4px">${['#ff6b5e', '#ffd60a', '#2df5b0', '#5aa9ff'].map((c) => `<i style="width:16px;height:16px;border-radius:50%;background:${c}"></i>`).join('')}</div>`);
    return box('linear-gradient(160deg,#4f8cff,#3b5bdb)', `<span style="font:800 22px var(--mono);color:#fff">&lt;/&gt;</span>`);
  };
  function clipCard(kind) {
    const c = T.s7cards[kind];
    const head = { otp: '#4fbf63', text: 'linear-gradient(90deg,#2e8f72,#3aa785)', image: '#3b7cf0', color: '#35b98b', code: '#4767e6' }[kind];
    let body = '';
    let foot = '';
    if (kind === 'otp') {
      body = `<div style="font:700 46px/1 var(--mono);letter-spacing:.08em;margin-top:6px">${c[2]}</div><div style="margin-top:14px;font:500 15px/1.55 var(--sans);color:var(--ink2)">${esc(c[3])}</div>`;
      foot = c[4];
    } else if (kind === 'text') {
      body = `<div style="font:600 21px/1.5 var(--sans);color:#f4f7f8">${esc(c[2])}</div><div style="margin-top:10px;font:500 15px var(--sans);color:var(--mint)">${esc(c[3])}</div>`;
      foot = c[4];
    } else if (kind === 'image') {
      body = `<div style="position:relative;height:150px;margin:-4px -6px 0;border-radius:10px;background:#15181c;padding:12px 14px;overflow:hidden;outline:2px solid #1e90ff">
          <div style="height:12px;width:80%;border-radius:4px;background:#e9edf1"></div><div class="ln" style="height:7px;width:92%;margin:12px 0 7px"></div><div class="ln" style="height:7px;width:84%;margin:7px 0"></div><div class="ln" style="height:7px;width:70%;margin:7px 0"></div>
          <div style="position:absolute;left:120px;top:36px;width:118px;height:18px;border:2px solid #ff3b30;border-radius:3px"></div>
          <div style="position:absolute;left:14px;top:92px;width:110px;height:56px;border-radius:6px;background:linear-gradient(180deg,#ffd29a,#f49a7c 55%,#6d5a96)"></div></div>`;
      foot = c[2];
    } else if (kind === 'color') {
      body = `<div style="position:absolute;left:0;right:0;top:66px;bottom:0;background:#2df5b0;display:grid;place-items:center;font:800 32px var(--mono);color:#0b2a20">${c[2]}<div style="position:absolute;bottom:16px;font:600 15px var(--sans);color:#14513d">${esc(c[3])}</div></div>`;
    } else {
      body = `<pre style="margin:0;font:500 16px/1.7 var(--mono);color:#d4d8de;white-space:pre"><span style="color:#ff7ab2">const</span> greet = (name: <span style="color:#5ad1ff">string</span>) =&gt;\n  <span style="color:#ffd27a">\`${esc(T.hello)}\${name}\`</span>;</pre>`;
      foot = c[2];
    }
    return `<div class="abs s7-card s7-${kind}" style="width:300px;height:280px;border-radius:20px;overflow:hidden;background:#2a2d31;box-shadow:0 14px 30px rgba(0,0,0,.35)">
      <div style="position:relative;height:66px;background:${head};padding:11px 18px"><div style="font:800 22px var(--sans)">${esc(c[0])}</div><div style="font:500 15px var(--sans);opacity:.85">${esc(c[1])}</div>${appIcon(kind)}</div>
      <div style="padding:16px 18px">${body}</div>
      ${foot ? `<div style="position:absolute;left:0;right:0;bottom:14px;text-align:center;font:500 15px var(--sans);color:var(--ink3)">${esc(foot)}</div>` : ''}
    </div>`;
  }
  const qrR = rng(2024);
  const S7 = scene(
    's7', 47.95, 58.1,
    `${hdrHTML('s7-hdr', '04', T.s7, 'style="width:940px"')}
     ${keysHTML('s7-keys', ['Alt', 'V'], T.s7hint, 120, 380)}
     <div class="win s7-login" style="left:${LOGIN.x}px;top:${LOGIN.y}px;width:${LOGIN.w}px;height:${LOGIN.h}px;background:#1b1e22">
       <div class="win-bar" style="height:44px"><span class="d"></span><span class="d"></span><span class="d"></span>dato.app/login</div>
       <div style="padding:30px 50px">
         <div style="display:flex;align-items:center;gap:14px;font:800 30px var(--sans)">${logoSVG('s7-login-logo', 40, { full: true }).replace('class="abs ', 'class="')}${esc(T.s7login[0])}</div>
         <div style="margin-top:22px;height:52px;border-radius:12px;background:rgba(255,255,255,.06);border:1px solid rgba(255,255,255,.1);display:flex;align-items:center;padding:0 18px;font:500 21px var(--sans);color:var(--ink2)">${esc(T.s7login[1])}</div>
         <div style="margin-top:18px;font:600 18px var(--sans);color:var(--ink3)">${esc(T.s7login[2])}</div>
         <div style="margin-top:10px;display:flex;gap:10px;align-items:center">${Array.from({ length: 6 }, () => `<div class="s7-box" style="width:56px;height:66px;border-radius:12px;background:rgba(255,255,255,.06);border:1.5px solid rgba(255,255,255,.14);display:grid;place-items:center;font:700 34px var(--mono)"></div>`).join('')}
           <div class="s7-btn" style="margin-left:auto;height:66px;padding:0 26px;border-radius:14px;background:#0a84ff;display:flex;align-items:center;gap:8px;font:700 22px var(--sans);white-space:nowrap"><span class="s7-btn-t">${esc(T.s7login[3])}</span></div></div>
       </div>
     </div>
     <div class="abs s7-qr glass" style="left:1080px;top:130px;width:300px;height:384px;border-radius:26px;padding:26px">
       <canvas class="s7-qrc" width="238" height="238" style="border-radius:12px;background:#fff;display:block"></canvas>
       <div class="abs s7-qrscan" style="left:26px;top:26px;width:238px;height:6px;background:linear-gradient(90deg,transparent,#2df5b0,transparent);box-shadow:0 0 20px #2df5b0"></div>
       <div style="margin-top:16px;text-align:center;font:600 19px var(--sans);color:var(--ink2)" class="s7-qrt">${esc(T.s7sync[0])}</div>
       <div class="s7-qrt2" style="margin-top:8px;text-align:center;font:500 17px var(--sans);color:var(--ink3)">${esc(T.s7sync[1])}</div>
     </div>
     <div class="abs s7-phone" style="left:1480px;top:40px;width:280px;height:540px;border-radius:46px;background:#0c0e10;border:7px solid #30353b;box-shadow:0 30px 80px rgba(0,0,0,.6);overflow:hidden;padding:56px 16px 16px">
       <div style="position:absolute;left:50%;top:14px;width:90px;height:26px;margin-left:-45px;border-radius:13px;background:#000"></div>
       <div style="font:800 26px var(--sans);padding:0 6px 12px">${esc(T.s7tabs[0])}</div>
       <div class="s7-pnew" style="height:0;overflow:hidden"><div style="height:84px;margin-bottom:10px;border-radius:16px;background:rgba(79,191,99,.22);border:1.5px solid #4fbf63;padding:12px 14px"><div style="font:600 14px var(--sans);color:#8be39b">${esc(T.s7cards.otp[0])} · ${esc(T.s7cards.otp[1])}</div><div style="font:700 30px var(--mono);letter-spacing:.06em;margin-top:2px">482913</div></div></div>
       ${['Simplicity is the ultimate…', '#2DF5B0', 'const greet = (name…', '880 × 574'].map((s) => `<div style="height:70px;margin-bottom:10px;border-radius:16px;background:rgba(255,255,255,.07);padding:12px 14px;font:500 17px var(--sans);color:var(--ink2);white-space:nowrap;overflow:hidden">${esc(s)}<div class="ln" style="width:50%;height:8px;margin:10px 0 0"></div></div>`).join('')}
     </div>
     <svg class="abs" width="${W}" height="${H}" style="pointer-events:none"><path class="s7-arc" fill="none" stroke="#2df5b0" stroke-width="3" stroke-dasharray="3 10" stroke-linecap="round"/><circle class="s7-arcdot" r="8" fill="#2df5b0" style="filter:drop-shadow(0 0 10px #2df5b0)"/></svg>
     <div class="abs s7-mail glass" style="left:1330px;top:28px;width:520px;border-radius:22px;padding:16px 20px;display:flex;gap:16px">
       <div style="flex:none;width:52px;height:52px;border-radius:13px;background:linear-gradient(160deg,#5fb4ff,#2f74e8);display:grid;place-items:center">${icon('mail', 28, '#fff')}</div>
       <div style="flex:1;min-width:0"><div style="display:flex;font:600 16px var(--sans);color:var(--ink3)">${esc(T.s7mail[0])}<span style="flex:1"></span>${esc(T.s7mail[1])}</div>
         <div style="font:700 20px var(--sans);margin-top:2px">${esc(T.s7mail[2])}</div>
         <div style="font:500 18px/1.45 var(--sans);color:var(--ink2);margin-top:2px">${esc(T.s7mail[3]).replace('482913', '<span class="s7-digits" style="border-radius:5px;padding:0 3px;font-family:var(--mono);font-weight:700">482913</span>')}</div></div>
     </div>
     <div class="abs s7-panel" style="left:${PNL.x}px;top:${PNL.y}px;width:${PNL.w}px;height:${PNL.h}px;border-radius:30px;background:rgba(24,30,32,.78);backdrop-filter:blur(30px) saturate(150%);border:1px solid rgba(255,255,255,.1);box-shadow:0 40px 100px rgba(0,0,0,.6)">
       <div style="position:absolute;left:28px;top:22px;right:28px;height:46px;display:flex;align-items:center;gap:16px">
         <div style="width:42px;height:42px;border-radius:50%;background:radial-gradient(circle at 35% 30%,#e8fbff,#7cc7ff 45%,#2a6fd6);box-shadow:inset 0 -4px 10px rgba(0,0,0,.25)"></div>
         <div style="width:330px;height:44px;border-radius:12px;background:rgba(255,255,255,.07);display:flex;align-items:center;gap:10px;padding:0 14px;font:500 19px var(--sans);color:var(--ink3)">${icon('search', 20)}${esc(T.s7search)}</div>
         <div style="flex:1;display:flex;justify-content:center;gap:12px;font:600 19px var(--sans);color:var(--ink2)">
           <span style="padding:8px 16px;border-radius:12px;background:rgba(255,255,255,.1);color:#fff">${esc(T.s7tabs[0])}</span>
           <span style="padding:8px 12px;display:flex;align-items:center;gap:8px"><i style="width:10px;height:10px;border-radius:50%;background:#5aa9ff"></i>${esc(T.s7tabs[1])}</span>
           <span style="padding:8px 12px;display:flex;align-items:center;gap:8px"><i style="width:10px;height:10px;border-radius:50%;background:#ffb340"></i>${esc(T.s7tabs[2])}</span>
           <span style="padding:8px 8px">+</span></div>
         ${icon('more', 26, 'rgba(240,244,247,.7)')}
       </div>
       <div class="s7-cards" style="position:absolute;left:0;top:92px;right:0;height:290px">
         ${clipCard('otp')}${clipCard('text')}${clipCard('image')}${clipCard('color')}${clipCard('code')}
         <div class="abs s7-ring" style="width:310px;height:290px;border-radius:24px;border:3.5px solid #0a84ff;box-shadow:0 0 30px rgba(10,132,255,.45)"></div>
       </div>
     </div>
     <div class="abs s7-fly" style="font:700 40px/1 var(--mono);letter-spacing:.08em;color:#2df5b0;text-shadow:0 0 20px rgba(45,245,176,.8);white-space:nowrap">482913</div>
     <div class="abs s7-copied toast glass"><span class="ok">${icon('check', 18, '#06130d', 3)}</span>${esc(T.s7copied)}</div>`,
    (t, sc) => {
      animHdr(sc, 's7-hdr', t, 48.0, 57.5);
      animKeys(sc, 's7-keys', t, 48.15, 48.5, 57.45);
      const panel = sc.q('.s7-panel');
      const pin = P(t, 48.5, 49.05, E.outBackSoft);
      const pout = P(t, 57.45, 57.95, E.inCubic);
      put(panel, { y: (1 - pin) * 480 + pout * 480, o: P(t, 48.5, 48.6) });

      // 卡片位置：验证码到来之前，四张卡占 0–3 号位；之后整体右移一位，验证码插在最前面
      const shift = P(t, 51.3, 51.7, E.outCubic);
      const order = ['text', 'image', 'color', 'code'];
      order.forEach((k, i) => {
        const c = sc.q('.s7-' + k);
        const a = 48.7 + i * 0.07;
        const p = P(t, a, a + 0.4, E.outCubic);
        put(c, { x: SLOT(i + shift), y: (1 - p) * 40, o: p });
      });
      const otp = sc.q('.s7-otp');
      const op = P(t, 51.72, 52.05, E.outBack);
      put(otp, { x: SLOT(0), s: 0.7 + 0.3 * op, o: P(t, 51.72, 51.8) });
      otp.style.boxShadow = t > 51.72 ? `0 14px 30px rgba(0,0,0,.35), 0 0 ${(60 * Math.exp(-(t - 51.75) * 2)).toFixed(1)}px rgba(79,191,99,${(0.9 * Math.exp(-(t - 51.75) * 2)).toFixed(3)})` : 'none';
      // 选中框：方向键一格一格挪
      const ring = sc.q('.s7-ring');
      let slot = 0;
      if (t >= 49.5) slot = lerp(0, 1, P(t, 49.5, 49.62, E.outCubic));
      if (t >= 50.0) slot = lerp(1, 2, P(t, 50.0, 50.12, E.outCubic));
      if (t >= 51.3) slot = lerp(2, 0, P(t, 51.68, 51.85, E.outCubic));
      const enter = press(t, 52.6, 0.06);
      put(ring, { x: SLOT(slot) - 5, y: -5, s: 1 - 0.03 * enter, o: P(t, 49.0, 49.15) });

      // 邮件通知 → 验证码飞进剪贴板
      const mail = sc.q('.s7-mail');
      const mp = P(t, 50.6, 51.0, E.outBackSoft);
      const mo = P(t, 52.2, 52.55, E.inCubic);
      put(mail, { x: (1 - mp) * 560 + mo * 560, o: P(t, 50.6, 50.7) });
      const code = sc.q('.s7-digits');
      const hlp = P(t, 51.0, 51.15);
      code.style.background = `rgba(45,245,176,${(0.3 * hlp).toFixed(3)})`;
      code.style.color = hlp > 0.5 ? '#2df5b0' : '';
      const fly = sc.q('.s7-fly');
      const fp = P(t, 51.2, 51.78, E.inOutCubic);
      if (t > 51.2 && t < 51.82) {
        const a = S7.L.code;
        const b = { x: PNL.x + SLOT(0) + 18, y: PNL.y + 92 + 66 + 22 };
        const x = lerp(a.x, b.x, fp);
        const y = lerp(a.y, b.y, fp) - Math.sin(fp * Math.PI) * 140;
        put(fly, { x, y, s: lerp(0.5, 1.1, fp), o: 1 });
      } else fly.style.opacity = 0;
      const cp = sc.q('.s7-copied');
      const cpp = P(t, 51.9, 52.2, E.outBack);
      cp.style.left = `${PNL.x + SLOT(0)}px`;
      cp.style.top = `${PNL.y - 74}px`;
      put(cp, { y: (1 - cpp) * 20, o: P(t, 51.9, 52.0) * (1 - P(t, 53.4, 53.7)) });

      // 回车：贴进登录框
      const login = sc.q('.s7-login');
      const lin = P(t, 48.3, 48.8, E.outCubic);
      const lout = P(t, 54.0, 54.4, E.inCubic);
      put(login, { y: (1 - lin) * 40 - lout * 40, o: lin * (1 - lout) });
      sc.qa('.s7-box').forEach((b, i) => {
        const a = 52.7 + i * 0.07;
        const on = t >= a;
        const ch = on ? '482913'[i] : '';
        if (b._c !== ch) {
          b.textContent = ch;
          b._c = ch;
        }
        const k = on ? Math.exp(-(t - a) * 6) : 0;
        b.style.borderColor = on ? `rgba(45,245,176,${(0.4 + 0.6 * k).toFixed(3)})` : 'rgba(255,255,255,.14)';
        b.style.transform = `scale(${(1 + 0.12 * k).toFixed(3)})`;
      });
      const btn = sc.q('.s7-btn');
      const ok = t >= 53.4;
      btn.style.background = ok ? '#30d158' : '#0a84ff';
      btn.style.transform = `scale(${(1 - 0.06 * press(t, 53.2, 0.05)).toFixed(3)})`;
      const bt = sc.q('.s7-btn-t');
      const btxt = ok ? '✓ ' + T.s7login[4] : T.s7login[3];
      if (bt._s !== btxt) {
        bt.textContent = btxt;
        bt._s = btxt;
      }

      // 多端同步：扫码 → 连上 → 手机上也有了
      const qr = sc.q('.s7-qr');
      const qp = P(t, 54.35, 54.8, E.outBackSoft);
      const allOut = P(t, 57.4, 57.9, E.inCubic);
      put(qr, { y: (1 - qp) * 40, s: 0.94 + 0.06 * qp, o: P(t, 54.35, 54.5) * (1 - allOut) });
      const scan = sc.q('.s7-qrscan');
      put(scan, { y: lerp(0, 232, P(t, 54.8, 55.3, E.inOutSine)), o: t > 54.8 && t < 55.35 ? 1 : 0 });
      const qrt = sc.q('.s7-qrt');
      const linked = t > 55.35;
      if (qrt._l !== linked) {
        qrt.innerHTML = linked ? `<span style="color:#2df5b0">${esc(T.s7linked)}</span>` : esc(T.s7sync[0]);
        qrt._l = linked;
      }
      const phone = sc.q('.s7-phone');
      const php = P(t, 54.45, 54.95, E.outBackSoft);
      put(phone, { x: (1 - php) * 120, o: P(t, 54.45, 54.6) * (1 - allOut) });
      const arc = sc.q('.s7-arc');
      if (!arc._len) {
        const a = { x: PNL.x + SLOT(0) + 150, y: PNL.y - 6 };
        const b = { x: 1478, y: 480 };
        arc.setAttribute('d', `M${a.x} ${a.y} C ${a.x + 500} ${a.y - 150}, ${b.x - 380} ${b.y + 110}, ${b.x} ${b.y}`);
        arc._len = arc.getTotalLength();
      }
      const ap = P(t, 55.4, 55.95, E.inOutCubic);
      arc.style.opacity = (ap > 0 ? 1 : 0) * (1 - P(t, 56.4, 56.8));
      arc.setAttribute('stroke-dasharray', makeDash(ap * arc._len, arc._len));
      const dot = sc.q('.s7-arcdot');
      const pt = arc.getPointAtLength(ap * arc._len);
      dot.setAttribute('cx', pt.x);
      dot.setAttribute('cy', pt.y);
      dot.style.opacity = ap > 0 && ap < 1 ? 1 : 0;
      sc.q('.s7-pnew').style.height = `${(94 * P(t, 55.95, 56.3, E.outBack)).toFixed(1)}px`;
      put(sc.q('.s7-qrt2'), { o: P(t, 56.2, 56.5) });
    },
  );
  afterLayout.push(() => {
    const c = rel(S7.q('.s7-digits'));
    S7.L = { code: { x: c.x, y: c.y } };
    // 二维码：三个定位角 + 随机模块
    const g = S7.q('.s7-qrc').getContext('2d');
    const n = 25;
    const m = 238 / (n + 2);
    g.fillStyle = '#fff';
    g.fillRect(0, 0, 238, 238);
    g.fillStyle = '#111';
    const finder = (fx, fy) => {
      g.fillRect((fx + 1) * m, (fy + 1) * m, 7 * m, 7 * m);
      g.fillStyle = '#fff';
      g.fillRect((fx + 2) * m, (fy + 2) * m, 5 * m, 5 * m);
      g.fillStyle = '#111';
      g.fillRect((fx + 3) * m, (fy + 3) * m, 3 * m, 3 * m);
    };
    for (let y = 0; y < n; y++) {
      for (let x = 0; x < n; x++) {
        const inF = (x < 8 && y < 8) || (x > n - 9 && y < 8) || (x < 8 && y > n - 9);
        if (!inF && qrR() > 0.52) g.fillRect((x + 1) * m, (y + 1) * m, m + 0.5, m + 0.5);
      }
    }
    finder(0, 0);
    finder(n - 7, 0);
    finder(0, n - 7);
    // 中间放个小图标
    g.fillStyle = '#fff';
    g.fillRect(238 / 2 - 22, 238 / 2 - 22, 44, 44);
    g.fillStyle = '#16191d';
    g.beginPath();
    g.roundRect(238 / 2 - 18, 238 / 2 - 18, 36, 36, 9);
    g.fill();
    const grd = g.createLinearGradient(238 / 2 - 12, 0, 238 / 2 + 12, 0);
    grd.addColorStop(0, '#e9ff5a');
    grd.addColorStop(1, '#2df5b0');
    g.fillStyle = grd;
    [[-8, 1, 18], [0, 3, 12], [8, 2, 15]].forEach(([dy, , len]) => {
      g.beginPath();
      g.roundRect(238 / 2 - 10, 238 / 2 + dy - 2.5, len, 5, 2.5);
      g.fill();
    });
  });
  cue(48.5, 'key');
  cue(48.5, 'whoosh', { dur: 0.45, soft: 1 });
  [48.7, 48.77, 48.84, 48.91].forEach((a, i) => cue(a, 'card', { n: i, soft: 1 }));
  cue(49.5, 'tick');
  cue(50.0, 'tick');
  cue(50.6, 'chime');
  cue(51.2, 'fly', { dur: 0.58 });
  cue(51.75, 'success');
  cue(52.6, 'key', { soft: 1 });
  for (let i = 0; i < 6; i++) cue(52.7 + i * 0.07, 'type');
  cue(53.2, 'click');
  cue(53.4, 'success', { soft: 1 });
  cue(54.35, 'pop', { soft: 1 });
  cue(54.8, 'scan', { dur: 0.5, soft: 1 });
  cue(55.35, 'blip', { n: 2 });
  cue(55.4, 'fly', { dur: 0.55 });
  cue(55.95, 'chime', { soft: 1 });

  // ═════════════════════════ S8 还有很多（58–64）→ S9 隐私（64–68） ═════════════════════════
  const LOCK = { cx: 600, cy: 540, step: 25 };
  const S8 = scene(
    's8', 57.95, 68.05,
    `<div class="abs s8-chips">${T.s8chips.map((c) => `<span class="abs chip s8-chip" style="font-size:24px;padding:12px 22px">${esc(c)}</span>`).join('')}</div>
     <div class="abs s8-center" style="left:0;top:0;width:${W}px;height:${H}px">${T.s8words.map(([w, sub]) => `<div class="abs s8-w" style="left:0;right:0;top:400px;width:${W}px;text-align:center"><div style="font:900 ${T.s8fs || 150}px/1.1 var(--sans);letter-spacing:-0.01em;background:linear-gradient(90deg,#f4f7f8 20%,#2df5b0 50%,#f4f7f8 80%);background-size:300% 100%;-webkit-background-clip:text;background-clip:text;color:transparent" class="s8-big">${esc(w)}</div><div style="margin-top:22px;font:600 34px var(--sans);color:var(--ink2)" class="s8-sub">${esc(sub)}</div></div>`).join('')}</div>
     <canvas class="abs s8-cv" width="${W}" height="${H}" style="filter:drop-shadow(0 0 14px rgba(45,245,176,.45))"></canvas>
     <div class="abs s9-txt" style="left:930px;top:330px;width:900px">
       <div class="s9-l1" style="font:900 72px/1.25 var(--sans)">${chars(T.s9[0])}</div>
       <div class="s9-l2" style="font:900 72px/1.25 var(--sans)">${chars(T.s9[1])}</div>
       <div class="s9-sub" style="margin-top:30px;font:500 28px/1.7 var(--sans);color:var(--ink2)">${T.s9sub}</div>
       <div class="s9-icons" style="margin-top:30px;display:flex;gap:16px">${[['wifiOff', T.s9chips[0]], ['shield', T.s9chips[1]], ['lock', T.s9chips[2]]].map(([k, s]) => `<span class="chip s9-chip">${icon(k, 20, '#2df5b0')}${esc(s)}</span>`).join('')}</div>
     </div>`,
    (t, sc) => {
      // 正中的大字：每一拍换一个
      sc.qa('.s8-w').forEach((w, i) => {
        const a = 58.0 + i * 0.5;
        const on = t >= a && t < a + 0.5 && t < 62.0;
        if (!on) {
          w.style.opacity = 0;
          return;
        }
        const p = P(t, a, a + 0.2, E.outCubic);
        const last = i === T.s8words.length - 1;
        const out = last ? P(t, 61.8, 62.0) : 0;
        put(w, { s: 1.18 - 0.18 * p + 0.03 * (t - a), o: p * (1 - out), blur: (1 - p) * 14 });
        w.querySelector('.s8-big').style.backgroundPosition = `${(100 - (t - a) * 200).toFixed(1)}% 0`;
        put(w.querySelector('.s8-sub'), { y: (1 - P(t, a + 0.06, a + 0.3, E.outCubic)) * 16, o: P(t, a + 0.06, a + 0.25) });
      });
      // 四周的功能标签一个个冒出来，然后缩成点
      const chips = sc.qa('.s8-chip');
      const n = chips.length;
      const collapse = (i) => P(t, 62.0 + (i % 7) * 0.03, 62.3 + (i % 7) * 0.03, E.inCubic);
      chips.forEach((c, i) => {
        const pos = S8.chipPos[i];
        const a = 58.0 + i * (3.6 / n);
        const p = P(t, a, a + 0.35, E.outBack);
        const dr = (t - a) * 6;
        const dx = ((pos.x + pos.w / 2 - 960) / 960) * dr;
        const dy = ((pos.y + 26 - 540) / 540) * dr;
        const cl = collapse(i);
        put(c, { x: pos.x + dx, y: pos.y + dy, s: (0.6 + 0.4 * p) * (1 - cl), o: P(t, a, a + 0.1) * (1 - cl) * (0.55 + 0.45 * Math.exp(-(t - a) * 1.5)) });
        c.style.color = t - a < 0.4 && t > a ? '#2df5b0' : '';
      });

      // 点：标签和大字化成点，飞去拼成一把锁
      const cv = sc.q('.s8-cv');
      const g = cv.getContext('2d');
      g.clearRect(0, 0, W, H);
      if (t > 62.0) {
        const conv = P(t, 67.3, 67.95, E.inExpo);
        for (const d of S8.lockDots) {
          const a0 = d.src === 'chip' ? 62.0 + (d.k % 7) * 0.03 + 0.25 : 61.9 + d.rnd * 0.15;
          if (t < a0) continue;
          const f = P(t, 62.45 + d.rnd * 0.35, 63.35 + d.rnd * 0.35, E.inOutCubic);
          let x = lerp(d.sx, d.x, f);
          let y = lerp(d.sy, d.y, f) - Math.sin(f * Math.PI) * 80 * (d.rnd2 - 0.5);
          // 锁成形后一阵呼吸光从下往上走
          const wave = t > 63.6 ? Math.max(0, Math.sin((t - 63.6) * 3 - (LOCK.cy + 260 - d.y) * 0.012)) : 0;
          let r = 8.5 * (0.6 + 0.4 * P(t, a0, a0 + 0.2)) * (1 + 0.18 * wave);
          x = lerp(x, 960, conv);
          y = lerp(y, 540, conv);
          r *= 1 - 0.6 * conv;
          const k = clamp((d.x - (LOCK.cx - 240)) / 480);
          const cr = Math.round(lerp(233, 45, k));
          const cg = Math.round(lerp(255, 245, k));
          const cb = Math.round(lerp(90, 176, k));
          g.fillStyle = `rgba(${cr},${cg},${cb},${(0.55 + 0.45 * f).toFixed(3)})`;
          g.beginPath();
          g.arc(x, y, r, 0, Math.PI * 2);
          g.fill();
        }
      }
      // 隐私文案
      const out = P(t, 67.15, 67.5, E.inCubic);
      revealChars(sc.q('.s9-l1'), t, 64.0, { stagger: 0.05, dur: 0.55, y: 40, out: 67.15, outDur: 0.35 });
      revealChars(sc.q('.s9-l2'), t, 64.45, { stagger: 0.05, dur: 0.55, y: 40, out: 67.15, outDur: 0.35 });
      const sp = P(t, 65.0, 65.5, E.outCubic);
      put(sc.q('.s9-sub'), { y: (1 - sp) * 16, o: sp * (1 - out) });
      sc.qa('.s9-chip').forEach((c, i) => {
        const a = 65.5 + i * 0.2;
        const p = P(t, a, a + 0.4, E.outBack);
        put(c, { y: (1 - p) * 16, o: P(t, a, a + 0.12) * (1 - out) });
      });
    },
  );
  afterLayout.push(() => {
    // 标签位置：避开中间的大字，互不重叠
    const r = rng(77);
    const placed = [];
    S8.chipPos = S8.qa('.s8-chip').map((c) => {
      const w = c.offsetWidth;
      const h = 52;
      for (let tries = 0; tries < 400; tries++) {
        const x = 60 + r() * (W - 120 - w);
        const y = 50 + r() * (H - 100 - h);
        const box = { x, y, w, h };
        const cx = x + w / 2;
        const cy = y + h / 2;
        if (cx > 380 && cx < 1540 && cy > 330 && cy < 720) continue;
        if (placed.some((b) => x < b.x + b.w + 26 && x + w + 26 > b.x && y < b.y + b.h + 22 && y + h + 22 > b.y)) continue;
        placed.push(box);
        return box;
      }
      const box = { x: r() * (W - w), y: r() * (H - h), w, h };
      placed.push(box);
      return box;
    });
    // 锁的点阵：照 lucide 的 lock 图标放大 20 倍采样
    const c = document.createElement('canvas');
    c.width = c.height = 480;
    const g = c.getContext('2d');
    g.scale(20, 20);
    g.fillStyle = '#fff';
    g.beginPath();
    g.roundRect(3, 11, 18, 11, 2);
    g.fill();
    g.strokeStyle = '#fff';
    g.lineWidth = 2.6;
    g.beginPath();
    g.moveTo(7, 11.5);
    g.lineTo(7, 7);
    g.arc(12, 7, 5, Math.PI, 0);
    g.lineTo(17, 11.5);
    g.stroke();
    g.globalCompositeOperation = 'destination-out';
    g.beginPath();
    g.arc(12, 15.4, 1.7, 0, Math.PI * 2);
    g.fill();
    g.fillRect(11.2, 15.4, 1.6, 3.6);
    const d = g.getImageData(0, 0, 480, 480).data;
    const rr = rng(5);
    const dots = [];
    const st = LOCK.step;
    for (let y = st / 2; y < 480; y += st) {
      for (let x = st / 2; x < 480; x += st) {
        if (d[((y | 0) * 480 + (x | 0)) * 4 + 3] > 128) dots.push({ x: LOCK.cx - 240 + x, y: LOCK.cy - 240 + y, rnd: rr(), rnd2: rr() });
      }
    }
    // 打乱顺序，前面的点从标签里来，剩下的从大字里散出来
    dots.sort((a, b) => a.rnd2 - b.rnd2);
    dots.forEach((p, i) => {
      if (i < S8.chipPos.length) {
        const cp = S8.chipPos[i];
        p.src = 'chip';
        p.k = i;
        p.sx = cp.x + cp.w / 2;
        p.sy = cp.y + cp.h / 2;
      } else {
        p.src = 'word';
        p.sx = 960 + (rr() - 0.5) * 640;
        p.sy = 480 + (rr() - 0.5) * 140;
      }
    });
    S8.lockDots = dots;
  });
  for (let i = 0; i < 8; i++) cue(58.0 + i * 0.5, 'word', { n: i });
  T.s8chips.forEach((_, i) => cue(58.0 + i * (3.6 / T.s8chips.length), 'tick', { soft: 1, n: i }));
  cue(62.0, 'implode', { dur: 0.45, soft: 1 });
  cue(62.45, 'swarm', { dur: 1.2 });
  cue(64.0, 'hit', { soft: 1 });
  cue(67.3, 'implode', { dur: 0.65 });

  // ═════════════════════════ S10 结尾：现在，按下 F1（68–78） ═════════════════════════
  const LG = { x: 860, y: 132, s: 200 };
  const S10 = scene(
    's10', 67.95, 78.01,
    `${logoSVG('s10-logo', LG.s)}
     <div class="caption s10-word" style="top:372px;font:800 128px/1 var(--sans);letter-spacing:-0.02em">DATO OCR</div>
     <div class="caption s10-tag" style="top:538px;font-size:36px;font-weight:600;color:var(--ink2)">${chars(T.s10tag)}</div>
     <div class="abs s10-chips" style="left:0;right:0;top:614px;display:flex;justify-content:center;gap:16px">${T.s10chips.map((c) => `<span class="chip s10-chip" style="font-size:22px">${esc(c)}</span>`).join('')}</div>
     <div class="abs s10-url" style="left:0;right:0;top:716px;display:flex;justify-content:center"><span class="s10-urlt" style="display:inline-flex;align-items:center;gap:14px;font:700 36px var(--mono);color:#f4f7f8">${icon('github', 34, '#f4f7f8', 1.8)}${esc(T.s10url)}</span></div>
     <div class="abs s10-now" style="left:0;right:0;top:880px;display:flex;justify-content:center;align-items:center;gap:22px;font:800 46px var(--sans)"><span class="s10-nowt">${chars(T.s10now)}</span><span class="key s10-key" style="height:76px;line-height:76px;min-width:88px;font-size:34px;padding:0 22px">F1</span></div>
     ${selHTML('s10-sel', 0.55)}
     <div class="abs s10-dimlab glass" style="padding:6px 12px;border-radius:9px;font:600 17px var(--mono);white-space:nowrap"></div>
     ${toolbarHTML('s10-tb')}
     <div class="abs s10-beam" style="width:160px;background:linear-gradient(90deg,transparent,rgba(45,245,176,.28) 85%,#eafff6 99%,transparent)"></div>
     <div class="abs s10-toast toast glass" style="font-size:24px"><span class="ok">${icon('check', 18, '#06130d', 3)}</span>${esc(T.s10toast)}<span style="font-family:var(--mono);color:var(--mint)">${esc(T.s10url)}</span></div>`,
    (t, sc) => {
      const logo = sc.q('.s10-logo');
      put(logo, { x: LG.x, y: LG.y });
      animLogo(sc, 's10-logo', t, 67.98, { from: [512, 192 + ((540 - LG.y) * 640) / LG.s] });
      const gl = 0.5 + 0.5 * Math.exp(-Math.max(0, t - 68.4) * 1.5);
      logo.style.filter = `drop-shadow(0 0 ${(36 * gl).toFixed(1)}px rgba(45,245,176,${(0.35 * gl).toFixed(3)})) drop-shadow(0 24px 50px rgba(0,0,0,.5))`;
      const word = sc.q('.s10-word');
      const wp = P(t, 68.55, 69.2, E.outQuart);
      word.style.clipPath = `inset(-20px ${((1 - wp) * 50).toFixed(2)}% -20px ${((1 - wp) * 50).toFixed(2)}%)`;
      put(word, { o: P(t, 68.55, 68.7), s: 1.04 - 0.04 * wp });
      revealChars(sc.q('.s10-tag'), t, 69.1, { stagger: 0.025, dur: 0.45 });
      sc.qa('.s10-chip').forEach((c, i) => {
        const a = 69.6 + i * 0.12;
        const p = P(t, a, a + 0.4, E.outBack);
        put(c, { y: (1 - p) * 18, o: P(t, a, a + 0.12) });
      });
      const url = sc.q('.s10-urlt');
      const up = P(t, 70.2, 70.7, E.outCubic);
      put(url, { y: (1 - up) * 18, o: up });
      revealChars(sc.q('.s10-nowt'), t, 71.3, { stagger: 0.06, dur: 0.45 });
      const key = sc.q('.s10-key');
      const kp = P(t, 71.6, 72.0, E.outBack);
      const d = press(t, 72.5, 0.14);
      const glow = t > 72.5 ? Math.exp(-(t - 72.5) * 2) : 0;
      key.style.opacity = P(t, 71.6, 71.75);
      key.style.transform = `translateY(${((1 - kp) * 24 + d * 8).toFixed(2)}px) scale(${(0.85 + 0.15 * kp).toFixed(3)})`;
      key.style.boxShadow = `0 ${(10 - 8 * d).toFixed(1)}px 0 #15171a, 0 ${(18 - 10 * d).toFixed(1)}px 34px rgba(0,0,0,.55), inset 0 1px 0 rgba(255,255,255,.16), inset 0 0 0 ${(1.5 * glow).toFixed(2)}px rgba(45,245,176,${(0.9 * glow).toFixed(3)}), 0 0 ${(60 * glow).toFixed(1)}px rgba(45,245,176,${(0.5 * glow).toFixed(3)})`;
      // 按下 F1：整个画面变成截图遮罩，框住网址，识字，复制
      const L = S10.L;
      const sel = sc.q('.s10-sel');
      const end = P(t, 75.5, 75.9);
      const A = { x: L.url.x - 26, y: L.url.y - 18 };
      const B = { x: L.url.x + L.url.w + 26, y: L.url.y + L.url.h + 18 };
      const dp = P(t, 72.8, 73.35, E.inOutCubic);
      let cur = null;
      let curType = 'cross';
      let curPress = 0;
      if (t > 72.5) {
        const r = t < 72.8 ? { x: A.x, y: A.y, w: 0, h: 0 } : { x: A.x, y: A.y, w: (B.x - A.x) * dp, h: (B.y - A.y) * dp };
        placeSel(sel, r, P(t, 72.5, 72.6) * (1 - end));
        for (const h of sel.children) h.style.opacity = t > 72.8 ? 1 : 0;
        if (t < 72.8) {
          const p = P(t, 72.52, 72.78, E.inOutCubic);
          cur = { x: lerp(1180, A.x, p), y: lerp(980, A.y, p) };
        } else if (t < 73.4) {
          cur = { x: r.x + r.w, y: r.y + r.h };
          curPress = t < 73.35 ? 1 : 0;
        }
        const lab = sc.q('.s10-dimlab');
        if (t > 72.8) {
          lab.textContent = `${Math.round(r.w)} × ${Math.round(r.h)}`;
          lab.style.left = `${r.x}px`;
          lab.style.top = `${r.y - 42}px`;
        }
        lab.style.opacity = t > 72.8 ? 1 - end : 0;
      } else sel.style.opacity = 0;
      const tb = sc.q('.s10-tb');
      const tbp = P(t, 73.38, 73.68, E.outBack);
      put(tb, { y: (1 - tbp) * 14, s: 0.94 + 0.06 * tbp, o: P(t, 73.38, 73.5) * (1 - end) });
      for (const b of sc.qa('.s10-tb .tb-btn')) {
        if (b.dataset.k === 'check') continue;
        const on = b.dataset.k === 'ocr' && t > 73.85;
        b.style.background = on ? 'rgba(45,245,176,.22)' : 'transparent';
        b.style.color = on ? '#2df5b0' : 'rgba(240,244,247,.86)';
      }
      if (t >= 73.4 && t < 75.6) {
        curType = 'arrow';
        const p = P(t, 73.45, 73.8, E.inOutCubic);
        cur = { x: lerp(B.x, L.ocr.x, p), y: lerp(B.y, L.ocr.y, p) };
        curPress = press(t, 73.85, 0.06);
      }
      if (cur) setCursor(curType, cur.x, cur.y, { press: curPress, o: 1 - P(t, 75.3, 75.6) });
      const beam = sc.q('.s10-beam');
      const bp = P(t, 73.9, 74.45, E.inOutSine);
      beam.style.left = `${A.x}px`;
      beam.style.top = `${A.y}px`;
      beam.style.height = `${B.y - A.y}px`;
      put(beam, { x: lerp(-160, B.x - A.x, bp), o: t > 73.9 && t < 74.5 ? 1 : 0 });
      const toast = sc.q('.s10-toast');
      if (!toast._w) toast._w = toast.offsetWidth;
      toast.style.left = `${960 - toast._w / 2}px`;
      toast.style.top = '988px';
      const tp = P(t, 74.55, 74.9, E.outBack);
      put(toast, { y: (1 - tp) * 24, s: 0.9 + 0.1 * tp, o: P(t, 74.55, 74.7) * (1 - P(t, 76.2, 76.6)) });
    },
  );
  afterLayout.push(() => {
    const L = (S10.L = {});
    L.url = rel(S10.q('.s10-urlt'));
    const A = { x: L.url.x - 26, y: L.url.y - 18 };
    const B = { x: L.url.x + L.url.w + 26, y: L.url.y + L.url.h + 18 };
    const tb = S10.q('.s10-tb');
    tb.style.left = `${B.x - tb.offsetWidth}px`;
    tb.style.top = `${B.y + 12}px`;
    const ob = rel(S10.q('.s10-tb .tb-btn[data-k="ocr"]'));
    L.ocr = { x: ob.x + ob.w / 2 + 6, y: ob.y + ob.h / 2 + 8 };
    void A;
  });
  cue(67.98, 'finale');
  flashAt(67.98, 0.3, 0.5);
  cue(68.55, 'shimmer', { dur: 0.7 });
  cue(70.2, 'tick');
  cue(71.6, 'pop', { soft: 1 });
  cue(72.5, 'key');
  cue(72.8, 'click');
  cue(73.35, 'click', { up: 1 });
  cue(73.38, 'pop', { soft: 1 });
  cue(73.85, 'click');
  cue(73.9, 'scan', { dur: 0.55 });
  cue(74.55, 'success', { big: 1 });

  // ═════════════════════════ 引擎 ═════════════════════════
  function seek(t) {
    cursorState = null;
    drawBg(t);
    for (const sc of SCENES) {
      const on = t >= sc.t0 && t < sc.t1;
      if (on !== sc.shown) {
        sc.node.style.visibility = on ? 'visible' : 'hidden';
        sc.shown = on;
      }
      if (on) sc.update(t, sc);
    }
    drawWipe(t);
    drawFlash(t);
    $('#black').style.opacity = P(t, 76.7, 77.85).toFixed(3);
    drawGrain(t);
    drawCursor();
  }

  async function init() {
    document.documentElement.lang = LANG === 'en' ? 'en' : 'zh-CN';
    document.body.classList.toggle('lang-en', LANG === 'en');
    // 所有用到的字都先让字体加载好（Noto Sans SC 按 unicode-range 分片，只会拉用到的那几片）
    const allText = JSON.stringify(COPY[LANG] || COPY.zh) + document.body.innerText + QUOTE.join('') + 'DATO OCR0123456789';
    await Promise.all(
      ['400', '500', '600', '700', '800', '900'].flatMap((w) => [
        document.fonts.load(`${w} 40px "Noto Sans SC"`, allText),
        document.fonts.load(`${w} 40px "Inter"`, allText),
      ]).concat([document.fonts.load('500 40px "JetBrains Mono"', allText), document.fonts.load('700 40px "JetBrains Mono"', allText)]),
    );
    await document.fonts.ready;

    quoteLayout = layoutText({ w: CARD.w, h: CARD.h, lines: QUOTE, maxW: CARD.w - 180, maxFs: 96 });
    quoteDots = halftone(quoteLayout.canvas, CARD.step, 3);
    quoteGrid = pixelGrid(CARD.w, CARD.h, CARD.step);
    const sx = 620;
    const sy = 340;
    for (const d of quoteDots) d.delay = Math.hypot(d.x - sx, d.y - sy) / 1500 + d.rnd * 0.35;
    for (const fn of afterLayout) fn();

    CUES.sort((a, b) => a.t - b.t);
    window.FILM = { seek, duration: DURATION, fps: FPS, cues: CUES, lang: LANG, ready: true };
    if (!RENDER) setupPlayer();
    else document.body.classList.add('render');
    seek(Number(Q.get('t') || 0));
  }
  function setupPlayer() {
    const stage = $('#stage');
    const fit = () => {
      const s = Math.min(innerWidth / W, innerHeight / H);
      stage.style.transform = `translate(${(innerWidth - W * s) / 2}px,${(innerHeight - H * s) / 2}px) scale(${s})`;
    };
    fit();
    addEventListener('resize', fit);
    // 配乐由 `node render.mjs --audio` 生成；没有也能看，只是没声音
    const music = $('#music');
    music.src = `build/soundtrack-${LANG}.wav`;
    const scrub = $('#scrub');
    const clock = $('#clock');
    const btn = $('#play');
    let playing = false;
    let t = Number(Q.get('t') || 0);
    let last = 0;
    const show = () => {
      seek(t);
      scrub.value = String(Math.round((t / DURATION) * 1000));
      clock.textContent = `${Math.floor(t / 60)}:${(t % 60).toFixed(1).padStart(4, '0')}`;
    };
    const loop = (now) => {
      if (!playing) return;
      if (music.readyState >= 2 && !music.paused) t = music.currentTime;
      else t += (now - last) / 1000;
      last = now;
      if (t >= DURATION) {
        t = DURATION;
        playing = false;
        btn.textContent = '▶';
      }
      show();
      requestAnimationFrame(loop);
    };
    const toggle = () => {
      playing = !playing;
      btn.textContent = playing ? '❚❚' : '▶';
      if (playing) {
        if (t >= DURATION) t = 0;
        music.currentTime = t;
        music.play().catch(() => {});
        last = performance.now();
        requestAnimationFrame(loop);
      } else music.pause();
    };
    btn.onclick = toggle;
    addEventListener('keydown', (e) => {
      if (e.code === 'Space') {
        e.preventDefault();
        toggle();
      }
    });
    scrub.oninput = () => {
      t = (Number(scrub.value) / 1000) * DURATION;
      music.currentTime = t;
      show();
    };
    show();
  }

  init();
})();
