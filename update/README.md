# 更新清单

DATO COR 的"检查更新"从这里读：

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
2. `node scripts/release.mjs`
3. GitHub、Gitee 各建一个发行版，标签 `v<版本>`，上传 `release/<版本>/DATO-COR_<版本>_x64-setup.exe`
4. 提交并推送 `update/latest.json`、`update/latest-cn.json`，Gitee 仓库同步

**第 3 步要在第 4 步之前**：清单一推上去，开着自动检查的用户就会去下载安装包。
