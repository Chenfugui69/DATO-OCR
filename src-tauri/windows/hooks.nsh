; 安装包钩子（Tauri NSIS 模板在对应位置 !insertmacro）。
;
; 品牌从 DATO COR 改名为 DATO OCR（2026-10）。安装包按产品名记安装位置、卸载信息和开始菜单快捷方式，
; 名字一改，从 0.3.0 升级上来会在旁边再装一份，开始菜单里出现两个。装完新版后把旧的 DATO COR
; 静默卸掉：只删程序文件、快捷方式、卸载信息和它的开机自启项，不删用户数据（数据目录由新版自己搬）。

!macro NSIS_HOOK_POSTINSTALL
  ReadRegStr $R0 SHCTX "Software\Microsoft\Windows\CurrentVersion\Uninstall\DATO COR" "InstallLocation"
  ${If} $R0 != ""
    ; 注册表里的路径带着引号
    StrCpy $R1 $R0 1
    ${If} $R1 == '"'
      StrCpy $R0 $R0 "" 1
      StrCpy $R0 $R0 -1
    ${EndIf}
    ${If} $R0 != $INSTDIR
    ${AndIf} ${FileExists} "$R0\uninstall.exe"
      ; _?= 让卸载程序就地运行（不复制到临时目录），这样 ExecWait 能等它跑完
      ExecWait '"$R0\uninstall.exe" /S _?=$R0'
      Delete "$R0\uninstall.exe"
      RMDir "$R0"
    ${EndIf}
  ${EndIf}
!macroend
