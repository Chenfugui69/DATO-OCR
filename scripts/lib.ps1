# 各验收脚本共用的准备工作。用 `. "$PSScriptRoot\lib.ps1"` 点源引入。

# 清掉上一轮留下的 CHENOCR 进程。
#
# 为什么必须做：残留进程会继续占着 F1 全局热键，新起的进程注册失败（日志里是
# "HotKey already registered"）。于是脚本按下的那次 F1 被**旧进程**接走，量出来
# 的是上一个版本的行为 —— 而脚本照样报 PASS。这种假通过极难发现：我已经被它
# 骗过一次，当时 verify-backdrop 中途报错退出，没走到收尾的 Stop-Process。
function Reset-Chenocr {
  $stale = Get-Process chenocr -ErrorAction SilentlyContinue
  if ($stale) {
    Write-Host "清理残留进程: $($stale.Id -join ', ')"
    $stale | Stop-Process -Force
    Start-Sleep -Milliseconds 800
  }
}

# 确认热键真的注册上了。没注册上的话后面所有按键都是白按。
function Assert-Hotkey($logDir) {
  $lines = Get-ChildItem $logDir -Filter *.log -ErrorAction SilentlyContinue |
    ForEach-Object { Get-Content $_.FullName }

  if ($lines -match 'HotKey already registered') {
    throw 'F1 热键被别的进程占着，本次按键会被它接走 —— 先清干净再跑。'
  }
}

# 把所有影响行为的环境变量显式设一遍。
#
# 这些脚本常常在同一个 shell 里连着跑，而环境变量是会留下来的：之前
# verify-clipboard 就意外继承了上一个脚本设的 CHENOCR_ALLOW_SELF_CAPTURE。
# 显式设成想要的值，比"没设过就是默认值"可靠。
function Set-ChenocrEnv([bool]$allowSelfCapture, [string]$backdropFormat = 'bmp') {
  $env:CHENOCR_ALLOW_SELF_CAPTURE = if ($allowSelfCapture) { '1' } else { '0' }
  $env:CHENOCR_BACKDROP_FORMAT = $backdropFormat
  $env:CHENOCR_LOG = 'chenocr_lib=debug,chenocr=debug,warn'
}
