# DATO COR

一款体积小巧、以体验为先的桌面截图工具：截图标注、贴图、长截图、文字识别、翻译、剪贴板历史。

Windows 优先（Windows 10 2004+），macOS 已留好平台抽象层，后续移植。界面遵循 Apple 设计语言。

---

## 当前进度

第一版功能在 Windows 上已全部实现，并在开发机（3840×2160 @150% 单屏）上逐项真机跑过：

| 功能 | 快捷键 | 状态 |
|---|---|---|
| 截图 + 标注（矩形/椭圆/箭头/画笔/马赛克/文字，画完能拖动、改大小）、复制/保存/贴图；选区框样式、压暗可调 | F1 | ✅ 真机验证 |
| 截图原位翻译：译文直接写在截图原来的位置，颜色跟原文一致 | 截图后 Ctrl+T | ✅ 真机验证 |
| 瞬间截屏：按下即抓整个桌面（带鼠标指针），一碰就消失的弹窗也截得到 | Shift+F1 | ✅ 真机验证 |
| GIF 录制：截图框好区域后录屏，带鼠标指针，录完自动保存并复制 | 截图后 Ctrl+G | ✅ 真机验证 |
| 长截图（手动滚动，自动拼接，懒加载图片 / 无限滚动也能接上，滚完进编辑窗口） | F2 | ✅ 真机验证（模拟窗口）|
| 截图识字（RapidOCR 离线，系统 OCR 兜底），框选后直接出结果，识字窗口内翻译 | F3 | ✅ 真机验证 |
| 剪贴板历史面板，选中即粘贴 | Alt+V | ✅ 真机验证 |
| 多端同步：局域网设备码（主机点同意）、手机扫码网页 / 快捷指令、WebDAV 网盘（坚果云、Alist 挂的 115 等）；收到的直接 Ctrl+V | 主窗口「多端同步」 | ✅ 联网测试 + 网页实测；两台真机之间没测 |
| 划词翻译：快捷键、选字后的悬浮按钮、按住 Alt / Ctrl 选字直接翻译；多个翻译源同时显示；面板可毛玻璃 | Ctrl+Alt+T | ✅ 真机验证 |
| AI 对话：自定义接口和密钥、检测并挑选模型；可带选中文字、识字结果、截图提问 | 托盘 / 可设热键 | ✅ 模拟服务验证 |
| 主窗口：截图库、识字记录、剪贴板管理、设置 | 托盘 / 双击图标 | ✅ |

**还没验证的**：双显示器（尤其混合 DPI、副屏负坐标）、在 Chrome / 微信 / VS Code / Excel 里长截图、
安装包装到中文路径。macOS 只有桩代码。详见 [`docs/实现交接.md`](docs/实现交接.md) 第 5 节。

## 文档

| 给谁看 | 文件 | 说明 |
|---|---|---|
| 你（人） | [`docs/方案总览.md`](docs/方案总览.md) | 这软件长什么样、怎么用。不含代码。 |
| 下一个 AI | [`docs/实现交接.md`](docs/实现交接.md) | **接手前必读。** 当前进度、与规格的偏离、"看着多余但删了会出事"的代码、踩过的坑、测试方法。 |
| 下一个 AI | [`docs/spec/`](docs/spec/) | 十份技术规格，从架构到像素值。 |
| 下一个 AI | [`docs/会话交接.md`](docs/会话交接.md) | 方案阶段的决策过程和依据。 |

交接给 AI 时把这句话丢给它：

> 请先阅读 `docs/实现交接.md`，特别是第 3 节"不能删的东西"，再阅读 `docs/spec/00-项目总纲.md`，然后继续完善 DATO COR。改动某个模块前，先读对应的规格文档。

---

## 开发

### 环境要求

- Node 20+（自带 corepack，不需要全局装 pnpm；版本锁在 `package.json` 的 `packageManager`）
- Rust 1.82+，MSVC 工具链
- Windows 10 2004+（`WDA_EXCLUDEFROMCAPTURE` 和 WGC 抓屏都要求这个版本）

### 第一次拉下代码

```bash
corepack pnpm install
corepack pnpm fetch-ocr      # 下载识字引擎 RapidOCR-json 到 src-tauri/resources/ocr/（约 32MB，不入库）
```

`fetch-ocr` 会校验每个文件的 SHA-256；GitHub 慢的话用环境变量 `CHENOCR_OCR_URL` 指向镜像。
**不先跑它，`tauri build` 会因为找不到资源目录而失败**，`tauri dev` 下识字会退回系统 OCR。

### 日常

```bash
corepack pnpm tauri dev      # 开发（Vite 1420 端口 + 调试版 Rust）
corepack pnpm tauri build    # 出 NSIS 安装包：src-tauri/target/release/bundle/nsis/
```

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

- `CHENOCR_ALLOW_SELF_CAPTURE=1`：让 DATO COR 自己的窗口（遮罩、面板、气泡、贴图）能被截图工具拍到。
  正常情况下它们都设了 `WDA_EXCLUDEFROMCAPTURE`，外部截图只能拍到一片空白，自动化测试时要开这个。
- `CHENOCR_TEST_NO_CLIPBOARD=1`：录 GIF 后不把文件放进剪贴板（自动化测试时用）。
- `CHENOCR_TEST_DATA_DIR=目录`：数据（设置、数据库、图片）放到这个目录下，不碰真正的历史。
- `CHENOCR_TEST_SYNC_LOOPBACK=1`：局域网同步服务只绑 127.0.0.1（不触发防火墙询问）、不做 mDNS 广播、加入申请自动同意。
  只绑回环时只有本机程序连得上，所以自动同意不会放外人进来。
- 日志在 `%APPDATA%\DATO COR\logs\`，开发模式同时打到终端。前端未捕获的错误也会转进日志。

## 代码结构

```
src/                          前端（React 18 + TypeScript）
  views/<窗口>/               每个窗口一个目录；所有窗口共用 index.html，按窗口 label 选视图
  views/annotate/             标注引擎（截图遮罩和编辑窗口共用）
  ui/                         基础控件（基于 Radix 原语，自绘 Apple 风格）
  lib/ipc.ts                  所有 Rust 命令的类型化封装
src-tauri/src/                后端（Rust）
  platform/                   唯一允许出现平台相关代码的地方（windows/ 实现，macos/ 留桩）
  capture/ longshot/ ocr/ translate/ clipboard/ …   各功能模块，与规格文档一一对应
  storage/                    SQLite（截图库、识字记录、剪贴板、加密的密钥）
scripts/fetch-ocr.mjs         下载识字引擎
scripts/test/                 真机测试用的 PowerShell 小工具（模拟键鼠、截屏、测试目标窗口），见交接文档第 7 节
```

数据都在 `%APPDATA%\DATO COR\`：`chenocr.db`、`screenshots/`、`clipboard/`、`ocr/`、`logs/`、`settings.json`。

## 核心决策速查

| 项 | 结论 |
|---|---|
| 技术框架 | Tauri v2 + React + TypeScript |
| 截图交互 | 对齐微信 Windows 版手感 |
| 界面风格 | Apple 设计语言，外框用 Windows 原生 Mica / 亚克力 |
| 文字识别 | RapidOCR-json 离线（随包约 32MB），失败时退回 Windows 系统 OCR |
| 翻译 | 内置免费源（必应、腾讯、谷歌）+ 可自填 DeepL / OpenAI 兼容密钥；用户排序，第一个是默认源；可走系统 / 自定义代理 |
| 长截图 | 只做手动滚动，自动拼接 |
| 剪贴板 | 默认全量记录，本地存储，底部横向卡片面板 |
| AI 对话 | OpenAI 兼容 / Anthropic 接口，用户自填地址和密钥，密钥 DPAPI 加密 |
| 依赖许可 | 不引入 GPL / AGPL |
| 开源协议 | 闭源免费 |
