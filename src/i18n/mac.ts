// macOS 上和 Windows 不一样的文案：系统叫法、默认热键、按键名。只列有差别的，其余沿用。

type Overrides = { [key: string]: string | Overrides };

export const macZhCN: Overrides = {
  sync: {
    lan: {
      note: '第一次打开时 macOS 会询问是否允许 DATO OCR 查找本地网络上的设备（开着系统防火墙的还会问是否接受传入连接），都要允许，其他设备才连得进来。',
    },
    options: {
      autoWriteDesc: '其他设备刚复制的内容，在本机按 ⌘V 就能粘贴；关掉的话只进历史记录',
    },
  },
  library: {
    emptyDesc: '按 ⌥1 截图，截过的图都会保存在这里',
  },
  ocr: {
    noJobDesc: '截图后点工具条上的"识字"，或按 ⌥3 框选后识字',
    rerunSystem: '用系统自带识字重试',
  },
  settings: {
    general: {
      closeToTray: '关闭窗口时留在菜单栏',
    },
    capture: {
      snapDesc: '拖动选区边缘靠近窗口边界时自动吸附，按住 ⌥ 临时关闭',
      gifFpsDesc: '截图后点工具条上的"录制 GIF"（⌘G）。帧率越高越流畅，文件也越大；最长录 60 秒',
    },
    ocr: {
      system: '系统自带识字',
      rapidMissing: 'macOS 版用系统自带的识字引擎（Vision），完全离线，识别内容不会上传。',
    },
    translate: {
      enterKey: '输入密钥（加密保存，主密钥放在钥匙串里）',
    },
    popup: {
      blurDesc: '面板后面的内容模糊着透出来',
    },
    ai: {
      note: '提问内容（包括选中的文字、截图）会发给你配置的 AI 服务。密钥加密保存在本机，主密钥放在钥匙串里。',
    },
    network: {
      note: '翻译和 AI 对话会联网。系统代理会读取 macOS 的代理设置，Clash 等软件开启"系统代理"后自动生效。',
    },
    clipboard: {
      panelStyle: '面板样式',
      blurDesc: '面板后面的内容模糊着透出来',
    },
    appearance: {
      glassDesc: '窗口透出后面内容的毛玻璃材质',
      transparencyOff: '系统开着"减少透明度"，毛玻璃效果自动停用。',
      powerSaver: '低电量模式下毛玻璃效果自动停用。',
    },
    about: {
      privacy: '剪贴板历史、截图和识字记录以明文保存在本机数据目录中，不做加密、不上传；翻译密钥加密保存，主密钥放在钥匙串里。',
    },
  },
};

export const macEnUS: Overrides = {
  sync: {
    lan: {
      note: 'The first time, macOS asks whether DATO OCR may find devices on your local network (and, with the firewall on, whether to accept incoming connections). Allow both so other devices can connect.',
    },
    options: {
      autoWriteDesc: 'Paste with ⌘V right after copying on another device. Off = history only',
    },
  },
  library: {
    emptyDesc: 'Press ⌥1 to capture. Every screenshot is kept here',
  },
  ocr: {
    noJobDesc: 'Use "Recognize text" in the capture toolbar, or press ⌥3',
    rerunSystem: 'Retry with the built-in OCR',
  },
  settings: {
    general: {
      closeToTray: 'Keep running in the menu bar when closed',
    },
    capture: {
      snapDesc: 'Selection edges snap to window borders. Hold ⌥ to disable',
      gifFpsDesc: 'Use "Record GIF" on the capture toolbar (⌘G). Higher is smoother but bigger; up to 60 seconds',
    },
    ocr: {
      system: 'Built-in OCR',
      rapidMissing: 'The macOS version uses the built-in text recognition (Vision). It runs fully offline and nothing is uploaded.',
    },
    translate: {
      enterKey: 'Enter API key (encrypted with a key kept in your Keychain)',
    },
    popup: {
      blurDesc: 'Blur what is behind the panel',
    },
    ai: {
      note: 'What you ask (including selected text and screenshots) is sent to the AI service you configure. API keys are encrypted with a key kept in your Keychain.',
    },
    network: {
      note: 'Translation and AI chat use the network. System proxy follows the macOS proxy settings, such as those set by Clash.',
    },
    clipboard: {
      panelStyle: 'Panel style',
      blurDesc: 'Blur what is behind the panel',
    },
    appearance: {
      glassDesc: 'Translucent material that lets what is behind the window through',
      transparencyOff: '"Reduce transparency" is on in System Settings, so translucency is disabled.',
      powerSaver: 'Translucency is disabled in Low Power Mode.',
    },
    about: {
      privacy: 'Clipboard history, screenshots and recognition results are stored unencrypted in the local data folder and never uploaded. Translation API keys are encrypted with a key kept in your Keychain.',
    },
  },
};
