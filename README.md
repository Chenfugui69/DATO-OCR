<p align="center">
  <img src="docs/readme/hero.png" alt="DATO OCR —— 截图、识字、翻译、剪贴板，一个小工具，一气呵成" width="100%">
</p>

<p align="center">
  <b>简体中文</b> · <a href="README.en.md">English</a>
</p>

<p align="center">
  <a href="https://github.com/Chenfugui69/DATO-OCR/releases/latest"><b>下载最新版</b></a>
  &nbsp;·&nbsp; Windows 10 / 11 &nbsp;·&nbsp; macOS 13+（Apple 芯片）&nbsp;·&nbsp; 免费
</p>

看到一段想留下的内容，通常要走四步：截下来、把字认出来、翻译一下、再贴到别处。
DATO OCR 把这四步做成了一个工具：按一个键开始，结果直接在剪贴板里。

---

## 截图：框选、标注，一步到位

<p align="center">
  <img src="docs/readme/capture.gif" alt="框选一块区域，画框、画箭头" width="880">
</p>

- **框一下就能用**：自动识别窗口边界，拖边缘会吸附；框完直接复制、保存，或者钉在屏幕上当贴图。
- **标注**：矩形、椭圆、箭头、画笔、荧光笔、序号、马赛克、文字、测量、水印，还有聚光灯和放大镜。画完还能拖动、改大小，常用的组合可以存成模板。
- **瞬间截屏**：按下就抓整个桌面，一碰就消失的菜单和弹窗也截得到。
- **长截图**：滚动页面，自动拼接成一张长图。
- **识别表格和公式**：表格直接盖回原来的位置，样子和原图几乎一样，像 Excel 一样框选、改字、复制，也能导出 Excel；公式原位显示、能改 LaTeX。选区里有几张表、或者表和公式混在一起，也会分开认。公式可以下载本地模型，不用 AI，一秒内认完。
- **录屏**：MP4、GIF、APNG 都能录，可以带系统声音、麦克风和摄像头，显示鼠标点击和按下的键；录完先剪辑再保存。
- **二维码和取色**：截图时顺手识别二维码 / 条码；取色时按 P，把颜色贴在桌面上随时复制。
- **原位翻译**：截图后一键翻译，译文直接写在图片原来的位置；中英混排各翻各的，本来就是目标语言的部分不动。译文和原文都能直接选字复制，可以指定用哪个翻译服务。
- **工具条随你排**：按钮顺序在设置里拖动调整，还可以换成磨砂玻璃样式。

## 识字和翻译：选中就有结果

<p align="center">
  <img src="docs/readme/translate.png" alt="划词翻译面板：多个翻译源同时显示" width="412">
</p>

- **截图识字**：框选后直接出文字，识别完自动复制。完全离线，图片不会上传。
- **划词翻译**：选中文字，按快捷键或点一下悬浮按钮，几个翻译源的结果同时列出来。
- **内置免费翻译源**：有道、必应、谷歌、腾讯，开箱即用；某个服务被限流或超时，接下来几分钟自动先用别的。
- **更多翻译服务**：DeepL、百度、彩云小译、小牛、微软 Azure、腾讯云、火山，填上自己的密钥就能用；还可以自己添加 AI 模型、DeepLX 或任意网络接口。拖动就能排顺序。
- **直接问 AI**：翻译面板底下就是输入框，带着选中的文字、识字结果或截图提问，也可以拖进、粘贴图片。回答边生成边显示。接口和密钥由你自己填。

## 剪贴板：复制过的都找得回来

<p align="center">
  <img src="docs/readme/clipboard.png" alt="剪贴板面板：验证码、文字、图片、颜色、代码卡片，按来源应用上色" width="100%">
</p>

- **一眼分清**：每张卡片标着类型和来源应用，颜色、代码、链接、图片各有各的样子；图片可以打开大图预览。
- **选中即粘贴**：按快捷键呼出，方向键选中、回车就贴到刚才的窗口里。
- **分组和智能推荐**：常用的内容分组收好；点一下水珠，按你正在做的事推荐接下来要粘贴的几条。
- **验证码自动复制**：连上邮箱（QQ、163、Gmail、iCloud、Proton，或自己填服务器），新邮件里的验证码自动进剪贴板；Mac 上 iPhone 转发来的短信验证码也一样，开着多端同步时其他电脑也能收到。
- **多端同步**：同一个 Wi-Fi 下扫码就能连，电脑上复制、手机上粘贴；也能用坚果云等网盘同步。
- **给 AI 工具用**：在设置里打开 MCP，Claude、Cursor 这类 AI 工具就能搜你的剪贴板和识字记录，也能帮你往剪贴板里加内容、整理分组（可以关掉，只让它查）。
- **只存在本机**：历史记录不上传，可以设置自动清理、应用黑名单。

## 一切都可以调

<p align="center">
  <img src="docs/readme/settings.png" alt="设置页" width="880">
</p>

选区框的颜色和粗细、工具条按钮的顺序、面板样式、毛玻璃、每一个快捷键、翻译源的顺序，都在设置里；调划词面板的外观时，旁边就有真实大小的预览。
界面有简体中文、繁體中文、English、日本語、한국어，默认跟随系统语言。

---

## 快捷键

| 功能 | Windows | macOS |
|---|---|---|
| 截图 | `F1` | `⌥1` |
| 长截图 | `F2` | `⌥2` |
| 截图识字 | `F3` | `⌥3` |
| 瞬间截屏 | `Shift+F1` | `⌥⇧1` |
| 剪贴板面板 | `Alt+V` | `⌥V` |
| 划词翻译 | `Ctrl+Alt+T` | `⌃⌥T` |
| 截图后：翻译 / 录 GIF / 贴图 / 保存 | `Ctrl+T` / `G` / `P` / `S` | `⌘T` / `G` / `P` / `S` |

全部可以在「设置 → 快捷键」里改。

## 下载和安装

到 [发行版页面](https://github.com/Chenfugui69/DATO-OCR/releases/latest) 下载。

**Windows**：下载 `DATO-OCR_<版本>_x64-setup.exe`，双击安装。之后有新版本会在设置里提示，一键升级。

**macOS（Apple 芯片）**：下载 `.dmg`，把 DATO OCR 拖进"应用程序"。

- 首次打开会被系统拦下：到「系统设置 → 隐私与安全性」点「仍要打开」。
- 截图需要「屏幕录制」权限，划词翻译和选中即粘贴需要「辅助功能」权限，读 iPhone 转发来的短信验证码需要「完全磁盘访问权限」。应用的「设置 → 系统权限」里可以直接跳过去授权，给完重启应用。
- 有新版本时在「设置 → 更新」里一键升级：下载、校验后自动换成新版本并重新打开，系统授权不用重给。更早的 Mac 版（0.3.2 及以前，以及 10 月 16 日之前下载的 0.3.3）还不会自己换，要手动下载一次新版。

## 隐私

- 识字在本机完成：Windows 用随包的离线引擎，macOS 用系统自带的识别。
- 截图、识字记录、剪贴板历史只存在本机，不上传。
- 翻译和 AI 对话会联网，内容发给你选的那个服务。密钥和邮箱授权码加密保存在本机。
- 识别表格默认在本机完成；识别公式下载了本地模型也在本机完成，没下载时才交给你设置的 AI。
- 智能推荐默认在本机计算；只有在设置里打开「智能推荐用 AI 模型」，才会把当前窗口认出的文字和最近剪贴板的摘要发给「AI 对话」里设置的服务商。
- MCP 服务只在本机监听，要带令牌才能访问，默认关闭。

---

# 给开发者

Tauri v2 + React + TypeScript + Rust。Windows 是主平台；macOS 版已移植，部分功能还没有真人逐项验证。

## 开发

### 环境要求

- Node 20+（自带 corepack，不需要全局装 pnpm；版本锁在 `package.json` 的 `packageManager`）
- Rust 1.82+（Windows 上用 MSVC 工具链）
- Windows 10 2004+（`WDA_EXCLUDEFROMCAPTURE` 和 WGC 抓屏都要求这个版本）
- 或 macOS 13+，装了 Xcode 或命令行工具。**项目别放在 exFAT / FAT 的盘上编译**，至少编译产物不能
  （原因和绕法见交接文档 9.4）

### 第一次拉下代码

```bash
corepack pnpm install
corepack pnpm fetch-ocr      # 下载识字引擎 RapidOCR-json 到 src-tauri/resources/ocr/（约 32MB，不入库）
```

`fetch-ocr` 会校验每个文件的 SHA-256；GitHub 慢的话用环境变量 `CHENOCR_OCR_URL` 指向镜像。
**不先跑它，`tauri build` 会因为找不到资源目录而失败**，`tauri dev` 下识字会退回系统 OCR。
macOS 不用跑 `fetch-ocr`：识字用系统自带的 Vision，包里不带引擎。

### 日常

```bash
corepack pnpm tauri dev      # 开发（Vite 1420 端口 + 调试版 Rust）
corepack pnpm tauri build    # Windows 出 NSIS 安装包：src-tauri/target/release/bundle/nsis/
corepack pnpm build:mac      # macOS 出 .app：…/release/bundle/macos/DATO OCR.app
                             # 比直接 tauri build 多一步本机签名：不然每打一次包，系统授权就要重给一遍（交接文档 9.4）
```

### 发版（检查更新用）

```bash
node scripts/release.mjs     # 打包 → 用更新私钥签名 → 生成 update/latest.json（国外）和 latest-cn.json（国内）
                             # 在哪台电脑上跑就发哪个平台：Windows 出 .exe，Mac 出 .app.tar.gz（自动更新用）和 .dmg；
                             # 同一个版本两台电脑各跑一次，清单会合并
```

发版前把三处版本号（`package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`）改成新版本，
写好 `update/notes/<版本>.json`（给不懂技术的用户看的更新简介）。签名私钥在 `~/.tauri/dato-cor-updater.key`，
**丢了就没法再发能自动更新的版本**，务必备份；它不在仓库里。脚本跑完照着提示：先把安装包传到 GitHub 和 Gitee
的发行版（标签 `v<版本>`），再提交推送 `update/` 下的两份清单。详见 [`update/README.md`](update/README.md)。

### 测试与检查

```bash
corepack pnpm test                                        # 前端：选区几何与坐标换算
corepack pnpm lint                                        # ESLint（类型感知）
cargo test  --manifest-path src-tauri/Cargo.toml          # Rust：拼接算法、分词、排版还原、设置迁移……
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets
```

抓屏、识字引擎这类要真机的测试默认跳过，手动跑：

```bash
cargo test --manifest-path src-tauri/Cargo.toml -- --ignored --nocapture
```

### 调试开关

- `CHENOCR_ALLOW_SELF_CAPTURE=1`：让 DATO OCR 自己的窗口（遮罩、面板、气泡、贴图）能被截图工具拍到。
  正常情况下它们都设了 `WDA_EXCLUDEFROMCAPTURE`，外部截图只能拍到一片空白，自动化测试时要开这个。
- `CHENOCR_TEST_NO_CLIPBOARD=1`：录 GIF 后不把文件放进剪贴板（自动化测试时用）。
- `CHENOCR_TEST_DATA_DIR=目录`：数据（设置、数据库、图片）放到这个目录下，不碰真正的历史。
- `CHENOCR_TEST_SYNC_LOOPBACK=1`：局域网同步服务只绑 127.0.0.1（不触发防火墙询问）、不做 mDNS 广播、加入申请自动同意。
  只绑回环时只有本机程序连得上，所以自动同意不会放外人进来。
- `CHENOCR_TEST_UPDATE_URL=http://127.0.0.1:端口/latest.json`：检查更新只查这个地址（允许 http），启动 3 秒后就查。
- `CHENOCR_TEST_UPDATE_NO_INSTALL=1`：点"更新"只下载、验签，不运行安装包、不退出。
- 日志在 `%APPDATA%\DATO OCR\logs\`（macOS：`~/Library/Application Support/DATO OCR/logs/`），开发模式同时打到终端。前端未捕获的错误也会转进日志。
- `chenocr --action=capture`（还有 `instant` / `longshot` / `ocr` / `clipboard` / `translate` / `ai` / `cancel`）：
  让已经在跑的实例直接执行一个功能，等同按热键。脚本和自动化测试用。
- 调试版另有 `--eval=窗口标签:脚本.js`（在页面里执行脚本）和 `CHENOCR_FAKE_MONITOR`（假装多一块屏），
  macOS 上量遮罩流畅度用 `scripts/test/mac-overlay-bench.sh`，见交接文档 9.5。

## 代码结构

```
src/                          前端（React 18 + TypeScript）
  views/<窗口>/               每个窗口一个目录；所有窗口共用 index.html，按窗口 label 选视图
  views/annotate/             标注引擎（截图遮罩和编辑窗口共用）
  ui/                         基础控件（基于 Radix 原语，自绘 Apple 风格）
  lib/ipc.ts                  所有 Rust 命令的类型化封装
src-tauri/src/                后端（Rust）
  platform/                   唯一允许出现平台相关代码的地方（windows/ 和 macos/ 两套实现）
  capture/ longshot/ ocr/ translate/ clipboard/ …   各功能模块，与规格文档一一对应
  storage/                    SQLite（截图库、识字记录、剪贴板、加密的密钥）
scripts/fetch-ocr.mjs         下载识字引擎
scripts/test/                 真机测试用的 PowerShell 小工具（模拟键鼠、截屏、测试目标窗口），见交接文档第 7 节
```

数据都在 `%APPDATA%\DATO OCR\`（macOS：`~/Library/Application Support/DATO OCR/`）：`chenocr.db`、`screenshots/`、`clipboard/`、`ocr/`、`logs/`、`settings.json`。

## 核心决策速查

| 项 | 结论 |
|---|---|
| 技术框架 | Tauri v2 + React + TypeScript |
| 截图交互 | 对齐微信 Windows 版手感 |
| 界面风格 | Apple 设计语言，外框用系统原生材质（Windows 的 Mica / 亚克力，macOS 的毛玻璃） |
| 文字识别 | Windows：RapidOCR-json 离线（随包约 32MB），失败时退回系统 OCR；macOS：系统自带的 Vision |
| 翻译 | 内置免费源（必应、腾讯、谷歌）+ 可自填 DeepL / OpenAI 兼容密钥；用户排序，第一个是默认源；可走系统 / 自定义代理 |
| 长截图 | 只做手动滚动，自动拼接 |
| 剪贴板 | 默认全量记录，本地存储，底部横向卡片面板 |
| AI 对话 | OpenAI 兼容 / Anthropic 接口，用户自填地址和密钥，密钥加密保存（Windows DPAPI；macOS 钥匙串里的主密钥） |
| 依赖许可 | 只用与 GPL-3.0 兼容的协议（MIT / Apache-2.0 / BSD / MPL-2.0 / GPL-3.0），不引入 AGPL |
| 开源协议 | GPL-3.0，免费 |

## 许可证

DATO OCR 以 [GNU 通用公共许可证第 3 版（GPL-3.0）](LICENSE) 发布：可以自由使用、修改、再分发，但分发修改后的版本时也必须以 GPL-3.0 公开源代码。
