# 更新清单

DATO OCR 的"检查更新"从这里读：

| 文件 | 谁读 | 下载地址指向 |
|---|---|---|
| `latest.json` | 国外渠道（`raw.githubusercontent.com/.../main/update/latest.json`） | GitHub 发行版 |
| `latest-cn.json` | 国内渠道（`gitee.com/.../raw/main/update/latest-cn.json`） | Gitee 发行版 |
| `notes/<版本>.json` | 发版脚本 | — |

两份清单都由 `node scripts/release.mjs` 生成，不要手改（签名、大小、地址都是算出来的）。
格式和 Tauri updater 的静态 JSON 一样，多一个 `changes`（新增 / 修复 / 改进）给设置页的更新简介用。

## 写更新简介（`notes/<版本>.json`）

给不懂技术的用户看：

- 一条一句话，说用户能感觉到的变化，不说怎么实现的
- 不写内部名词（WGC、SSE、DPAPI…），写"截图"、"剪贴板"、"同步"
- 新增写"能做什么了"，修复写"以前哪里不对"
- `en` 里放英文版，英文界面会用它

```json
{
  "added": ["截图可以录成 GIF 动图"],
  "improved": ["剪贴板里的图片预览更清楚了"],
  "fixed": ["用取色器改颜色后保存失败"],
  "en": { "added": ["Record a screen area as a GIF"], "improved": [], "fixed": [] }
}
```

## 发版顺序

1. 改三处版本号，写 `notes/<版本>.json`
2. Windows 电脑上 `node scripts/release.mjs`：出 `DATO-OCR_<版本>_x64-setup.exe`，清单里写 `windows-x86_64`
3. 拉下第 2 步生成的清单，Mac 上再跑一次 `node scripts/release.mjs`：出 `DATO-OCR_<版本>_macos-arm64.app.tar.gz`（自动更新用）
   和 `.dmg`（手动下载用），清单里补上 `darwin-aarch64`。同一个版本的条目会合并，版本号不同就只留这次的
4. GitHub、Gitee 各建一个发行版，标签 `v<版本>`，上传 `release/<版本>/` 下的 `.exe`、`.app.tar.gz`、`.dmg`
5. 提交并推送 `update/latest.json`、`update/latest-cn.json`，Gitee 仓库同步

两台电脑用同一把更新私钥（`~/.tauri/dato-cor-updater.key`），换钥匙的话已装的版本都验不过新版。

Mac 的自动更新：下载 `.app.tar.gz` → 验签 → 解开后核对包标识、版本号和代码签名 → 在原来的文件夹里换掉旧的 .app →
退出并重新打开。`.app` 用 `scripts/mac-sign.mjs` 固定了签名身份，换完以后屏幕录制、辅助功能的授权还在。
Mac 自动更新从 2026-10-16 重新打的 0.3.3 起才有。0.3.2 及以前、以及更早下载的 0.3.3 没有这段代码：清单里有了
`darwin-aarch64` 以后，它们能看到新版本，但点「更新」会在下载后报错。所以 0.3.3 之后的第一版，更新简介里要写一句
"Mac 版如果点更新报错，请到发行版页面手动下载一次，以后就能自动更新了"。

**第 4 步要在第 5 步之前**：清单一推上去，开着自动检查的用户就会去下载安装包。

Gitee 还没准备好时用 `node scripts/release.mjs --github-only`：只生成 `latest.json`。国内渠道读不到 `latest-cn.json`
会自动改走 GitHub。Gitee 准备好以后，Gitee 发行版上传同一个安装包，再跑一次 `--skip-build` 生成两份清单。
脚本每次都会用程序里内置的公钥把签名验一遍，验不过就停下，不会发出签错钥匙的版本。
