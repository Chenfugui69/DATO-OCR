<p align="center">
  <img src="docs/readme/hero-en.png" alt="DATO OCR — capture, read, translate, paste: one small tool, one smooth move" width="100%">
</p>

<p align="center">
  <a href="README.md">简体中文</a> · <b>English</b>
</p>

<p align="center">
  <a href="https://github.com/Chenfugui69/DATO-OCR/releases/latest"><b>Download the latest release</b></a>
  &nbsp;·&nbsp; Windows 10 / 11 &nbsp;·&nbsp; macOS 13+ (Apple silicon) &nbsp;·&nbsp; Free
</p>

Keeping something you see on screen usually takes four steps: capture it, turn it into text, translate it, paste it somewhere.
DATO OCR makes those four steps one tool: press a key to start, and the result is already on your clipboard.

---

## Capture: select, annotate, done

<p align="center">
  <img src="docs/readme/capture.gif" alt="Select an area, then draw a box and an arrow" width="880">
</p>

- **Select and go**: windows are detected automatically and edges snap as you drag. Copy, save, or pin the result on screen.
- **Annotate**: rectangle, ellipse, arrow, pen, mosaic, text. Everything can be moved and resized afterwards.
- **Instant capture**: grabs the whole desktop the moment you press the key, including menus and popups that vanish on touch.
- **Scrolling capture**: scroll the page and it stitches one long image.
- **Record a GIF**: select an area and record it; it is saved, copied, and shows up in the screenshot library.
- **Translate in place**: one key after capturing, and the translation appears right where the original text was. Mixed-language lines are translated separately, and text already in the target language is left alone. Both the translation and the original can be selected and copied, and you can choose which service to use.
- **Your toolbar, your way**: drag the buttons into the order you like in Settings, or switch the toolbar to frosted glass.

## Recognize and translate: results as soon as you select

<p align="center">
  <img src="docs/readme/translate-en.png" alt="Selection translate popup with several services side by side" width="412">
</p>

- **Text recognition**: select an area and get the text, copied automatically. Fully offline; images are never uploaded.
- **Selection translate**: select text, press the shortcut or click the floating button, and several services answer side by side.
- **Free services built in**: Youdao, Bing, Google and Tencent work out of the box. If one is rate-limited or times out, the others go first for the next few minutes.
- **More services**: DeepL, Baidu, Caiyun, Niutrans, Microsoft Azure, Tencent Cloud and Volcengine with your own key — or add your own AI model, DeepLX, or any web API. Drag to reorder.
- **Ask AI**: the input box sits under the translation. Ask about the selected text, the recognized text, or the screenshot, and drop or paste in images. Answers stream in as they are written. You bring your own API address and key.

## Clipboard: everything you copied, still there

<p align="center">
  <img src="docs/readme/clipboard-en.png" alt="Clipboard panel: verification code, text, image, color and code cards, tinted by source app" width="100%">
</p>

- **Easy to tell apart**: every card shows its type and the app it came from; colors, code, links and images each get their own look, and images open in a full-size preview.
- **Paste on select**: open it with a shortcut, pick with the arrow keys, press Enter, and it lands in the window you were using.
- **Groups and smart picks**: keep things you reuse in groups. Tap the orb and it suggests what you are likely to paste next, based on what you are doing.
- **Verification codes, copied for you**: connect your mailbox (QQ, 163, Gmail, iCloud, Proton, or your own server) and codes in new emails go straight to the clipboard. On a Mac, SMS codes forwarded from your iPhone work the same way, and with sync on your other computers get them too.
- **Sync across devices**: scan a QR code on the same Wi-Fi to copy on the computer and paste on the phone, or sync through a WebDAV drive.
- **For AI tools**: turn on MCP in Settings and tools like Claude or Cursor can search your clipboard and recognition history, and add items or organize groups for you (you can make it read-only).
- **Stays on your machine**: history is never uploaded. Auto cleanup and an app blacklist are available.

## Everything is adjustable

<p align="center">
  <img src="docs/readme/settings-en.png" alt="Settings" width="880">
</p>

Selection frame color and thickness, toolbar button order, panel style, frosted glass, every shortcut, the order of translation services — all in Settings. When you style the selection translate popup, a full-size preview sits right next to it.
The interface is available in English and Simplified Chinese and follows your system language by default.

---

## Shortcuts

| Action | Windows | macOS |
|---|---|---|
| Capture | `F1` | `⌥1` |
| Scrolling capture | `F2` | `⌥2` |
| Recognize text | `F3` | `⌥3` |
| Instant capture | `Shift+F1` | `⌥⇧1` |
| Clipboard panel | `Alt+V` | `⌥V` |
| Selection translate | `Ctrl+Alt+T` | `⌃⌥T` |
| After capturing: translate / record GIF / pin / save | `Ctrl+T` / `G` / `P` / `S` | `⌘T` / `G` / `P` / `S` |

All of them can be changed in Settings → Shortcuts.

## Download and install

Get it from the [releases page](https://github.com/Chenfugui69/DATO-OCR/releases/latest).

**Windows**: download `DATO-OCR_<version>_x64-setup.exe` and run it. New versions are announced in Settings and install with one click.

**macOS (Apple silicon)**: download the `.dmg` and drag DATO OCR into Applications.

- The first launch is blocked by the system: go to System Settings → Privacy & Security and click "Open Anyway".
- Capturing needs the Screen Recording permission; selection translate and paste-on-select need Accessibility; reading SMS codes forwarded from your iPhone needs Full Disk Access. Settings → Permissions in the app takes you straight there. Restart the app after granting.
- The macOS version does not update itself yet; download new versions manually.

## Privacy

- Text recognition runs on your machine: the bundled offline engine on Windows, the system's own recognizer on macOS.
- Screenshots, recognition history and clipboard history are stored locally and never uploaded.
- Translation and AI chat go online and send the content to the service you chose. API keys and mailbox passwords are stored encrypted on your machine.
- Smart picks are computed on your machine by default. Only if you turn on "Use an AI model for suggestions" are the recognized window text and summaries of recent clips sent to the provider set in AI Chat.
- The MCP server is off by default, listens only on your own machine, and requires a token.

---

## For developers

Tauri v2 + React + TypeScript + Rust. Windows is the primary platform; the macOS port is in place, with some features not yet verified by hand.
Build instructions, debug switches and the code map are in the [Chinese README](README.md#给开发者).

```bash
corepack pnpm install
corepack pnpm fetch-ocr      # Windows only: downloads the offline OCR engine
corepack pnpm tauri dev
```

## License

DATO OCR is released under the [GNU General Public License v3.0](LICENSE). You may use, modify and redistribute it, but any modified version you distribute must also be released under GPL-3.0 with its source code.
