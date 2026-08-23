# CHENOCR

一款体积小巧、以体验为先的桌面截图工具。截图、长截图、文字识别、翻译、剪贴板历史，五件事做透。

Windows 优先，macOS 后续移植。界面遵循 Apple 设计语言。

---

## 当前进度

**M0 · 技术验证**（规格 00 §5）。截图主链路已经跑通：F1 → 抓屏 → 原生冻结底图 +
全屏透明遮罩 → 拖拽框选 → Enter 裁剪进系统剪贴板 / Esc 退出。

在 3840×2160 @150% 单屏上：热键到画面可见首次 79ms、后续 67–72ms（预算 150ms），
底图与真实桌面逐像素零差异，剪贴板尺寸精确。**这些数字只对单屏成立** —— 抓屏逐块
进行、底图每块屏各自上屏，两条都随屏数涨，第二块屏到货后要重测，不要外推。

**还缺**混合 DPI 双屏、副屏负坐标、双屏延迟复测（都等第二块屏），以及 ESLint 配置。
Clippy 已补（`unwrap_used` / `expect_used`）。清单见
[`docs/spec/08-开发排期与验收.md`](docs/spec/08-开发排期与验收.md)。

后续里程碑（M1 截图核心 → M7 打包发布）见同一份文档。

## 文档

| 给谁看 | 文件 | 说明 |
|---|---|---|
| 你（人） | [`docs/方案总览.md`](docs/方案总览.md) | 一口气讲清楚这软件长什么样、怎么用、分几步做完。不含代码。 |
| 下一个 AI | [`docs/实现交接-M0.md`](docs/实现交接-M0.md) | **接手实现前必读。** 当前进度、与规格的偏离、一批"看着多余但删了会出事"的代码、踩过的坑、性能基线。 |
| 下一个 AI | [`docs/spec/`](docs/spec/) | 十份技术规格，从架构到像素值全部写死，照着做就行。 |

交接给 AI 时把这句话丢给它：

> 请先阅读 `docs/实现交接-M0.md` 了解当前实现进度和其中第 3 节列出的"不能删的东西"，再阅读 `docs/spec/00-项目总纲.md`，按其中的开发顺序继续实现 CHENOCR。每开始一个模块前，先读对应的规格文档。

---

## 开发

### 环境要求

- Node 20+
- Rust 1.82+（MSVC toolchain）
- Windows 10 2004+（`WDA_EXCLUDEFROMCAPTURE` 和 WGC 抓屏都要这个版本以上）

包管理器用 pnpm。仓库里没有全局安装 pnpm，靠 Node 自带的 corepack 跑，
版本锁在 `package.json` 的 `packageManager` 字段：

```bash
corepack pnpm install
corepack pnpm tauri dev      # 开发
corepack pnpm tauri build    # 出安装包
```

### 测试

```bash
corepack pnpm test                                    # 前端：选区几何与坐标换算
cargo test --manifest-path src-tauri/Cargo.toml       # Rust：拼接、裁剪、BMP 编码
```

抓屏的真机烟雾测试要有物理显示器，默认跳过。手动跑它会打印各显示器的几何、
缩放比例和分段耗时 —— 排查 DPI 和性能问题先看这个：

```bash
cargo test --manifest-path src-tauri/Cargo.toml -- --ignored --nocapture
```

量性能用 `perf` 档位（和 release 同样开优化，但不做全量 LTO，重编快得多）：

```bash
cargo test --profile perf --manifest-path src-tauri/Cargo.toml -- --ignored --nocapture
```

### 验收脚本

`scripts/` 下的 PowerShell 脚本用来跑规格 08 里那些"得真按一下键才算数"的验收项。
都需要先 `corepack pnpm tauri build --no-bundle`（**不能**用 `cargo build` 直接编 ——
那样出来的包会指向 Vite 开发服务器，遮罩页面根本加载不出来）：

```powershell
# 量热键到遮罩可见的延迟：连开三轮，打印各阶段耗时
powershell -File scripts/measure-m0.ps1 <exe 路径>

# 底图是不是"冻结的、与真实桌面逐像素一致"的画面（规格 08 M0 那条背景一致性）
powershell -STA -File scripts/verify-backdrop.ps1 <exe 路径>

# 核对 Enter 复制出的图尺寸是否与选区严格一致，并检查进程有无残留
powershell -STA -File scripts/verify-clipboard.ps1 <exe 路径>

# 底图窗口的 z 序波段归属（直接读 GWL_EXSTYLE，不靠推断）
powershell -File scripts/probe-zorder.ps1 <exe 路径> [nostyle]

# 热身状态失效后会不会自动重新热身
powershell -File scripts/verify-rewarm.ps1 <exe 路径>

# 把遮罩截下来人眼看一眼
powershell -STA -File scripts/shoot-overlay.ps1 <exe 路径> [输出.png]
```

写这类脚本有三个已经踩过的坑，细节写在各脚本注释里：

1. **脚本自己必须声明 Per-Monitor-V2 DPI 感知**，否则量出来的坐标全是被系统
   虚拟化过的。这台开发机是 3840×2160 @150%，DPI 不感知的进程会把它报成
   2560×1440，选区尺寸对不上，还会让人误判显示器规格。
2. **脚本必须存 UTF-8 BOM**，否则 PowerShell 按 ANSI 解释中文注释，报出
   `MissingEndCurlyBrace` 这种跟真实原因毫无关系的语法错。
3. **像素比对脚本在"取基准"和"触发冻结"之间不能让屏幕有任何变化**，连一句
   `Write-Host` 都不行（它滚动终端，而比对区往往就压在终端上）。输出要攒到比对
   结束后再打。并且要带**对照组**先证明"该被看到的东西确实抓得到"，否则脚本
   可能因为压根没生效而假过 —— 假过比失败更危险。

### 图标

图标不以二进制形式入库，由脚本按数学定义画出来：

```bash
node scripts/generate-icons.mjs        # 生成源图 + 托盘图标
corepack pnpm tauri icon scripts/app-icon.png
```

### 代码结构里几个不明显的地方

- **平台相关代码只允许出现在 `src-tauri/src/platform/`**。这一层之外禁止
  `#[cfg(windows)]`、禁止 `use windows::`。理由见规格 07 §1。
- **冻结底图不进 WebView**，它是一个独立的原生分层窗口，和透明遮罩由 DWM 合成，
  两者用 `DeferWindowPos` 原子上屏。把底图送进 WebView 实测要 244ms（4K），
  预算只有 150ms；原生层是 70ms。完整理由见 `platform/mod.rs` 里 `BackdropLayer`
  的文档和规格 01 §2.4。
- **但底图像素照样要流进 WebView**，只是挪出了关键路径：放大镜、取色、马赛克需要
  原始像素。走自定义 `shot:` 协议，不落盘、不走 IPC，编码是 24bpp BMP
  （PNG 对桌面截图又慢又大，实测比 BMP 还大）。
- **遮罩窗口在启动时就建好并隐藏**，热键路径上只 `show`。WebView2 首次创建加载
  要 400ms 以上，放在热键之后必然爆预算。
- **抓屏必须开 xcap 的 `wgc` feature**。默认的 GDI 路径在 4K 屏上实测
  260–370ms，光抓屏就超预算了。
- **启动后要空跑一次抓屏预热**（`ScreenCapture::warm_up`）。首次抓屏要建 D3D 设备、
  开 WGC 会话，比后续贵 90ms（137ms vs 49ms），刚好让第一次 F1 冲破预算。
- **热身状态会失效，要在失效时机重新热身**（`capture::warmup`）。插拔屏、改分辨率、
  睡眠唤醒、显卡驱动 TDR 之后 GPU 侧状态可能已经没了，而症状是沉默的 —— 用户偶然
  遇到一次慢 F1，日志里只有 `capture_ms` 变大。TDR 拿不到 xcap 内部的 D3D 设备，
  只能间接覆盖，所以另外加了一条"抓屏慢得像冷启动"的 warn 把它暴露出来。
- **选区状态一律存虚拟桌面物理像素**，只在渲染时换算成 CSS 像素。
  混合 DPI 多屏下反过来做会累积误差。

## 核心决策速查

| 项 | 结论 |
|---|---|
| 技术框架 | Tauri v2 + React + TypeScript |
| 预期体积 | 安装包 25–40 MB，安装后 40–60 MB |
| 截图交互 | 100% 对齐微信 Windows 版手感 |
| 界面风格 | Apple Liquid Glass，外框走 Windows 原生亚克力 |
| 文字识别 | RapidOCR 离线（内置 30 MB），可选下载高精度引擎 |
| 翻译 | 内置免费源，支持自填 API Key |
| 长截图 | 手动滚动 + 自动拼接 |
| 剪贴板 | 全量记录本地存储，底部横向卡片面板 |
| 快捷键 | F1 截图 / F2 长截图 / F3 识字 / Alt+V 剪贴板 |
| 开源协议 | 闭源免费 |
| 第一版工期 | 约 10 周 |
