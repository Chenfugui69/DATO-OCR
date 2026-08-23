# M0 延迟分段测量。跑 release 版，用合成输入按 F1/Esc，把日志里的五段耗时抓出来。
#
#   .\scripts\measure-m0.ps1 <exe 路径> [bmp|png]
#
# 第二个参数选底图编码格式（默认 bmp），用来做 A/B 对比：BMP 编码快但字节多，
# PNG 反过来。两种都在关键路径上，谁快只能实测。
#
# 不属于产品代码，M0 收尾后可以删。

$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\lib.ps1"

$exe = $args[0]
$format = if ($args.Count -ge 2) { $args[1] } else { 'bmp' }
$logDir = Join-Path $env:APPDATA 'CHENOCR\logs'

if (-not $exe) { throw 'usage: measure-m0.ps1 <exe> [bmp|png]' }
if ($format -notin @('bmp', 'png')) { throw "unknown format: $format" }

Reset-Chenocr

Add-Type -Namespace Win32 -Name Input -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern void keybd_event(byte vk, byte scan, uint flags, System.IntPtr extra);
'@

function Send-Key([byte]$vk) {
  [Win32.Input]::keybd_event($vk, 0, 0, [IntPtr]::Zero)
  Start-Sleep -Milliseconds 40
  [Win32.Input]::keybd_event($vk, 0, 2, [IntPtr]::Zero)
}

if (Test-Path $logDir) { Remove-Item $logDir -Recurse -Force }

Set-ChenocrEnv $false $format

$proc = Start-Process -FilePath $exe -PassThru
Write-Host "已启动 pid=$($proc.Id)  format=$format，等待托盘就绪..."
Start-Sleep -Seconds 6
Assert-Hotkey $logDir

# 连开三轮：第一轮含 WGC 冷启动，后两轮才是稳定值。
foreach ($i in 1..3) {
  Write-Host "--- 第 $i 轮：F1 ---"
  Send-Key 0x70            # VK_F1
  Start-Sleep -Milliseconds 2500
  Send-Key 0x1B            # VK_ESCAPE
  Start-Sleep -Milliseconds 1200
}

# 先读日志再杀进程。tracing-appender 是非阻塞写，Stop-Process -Force
# 会把还在队列里的行直接丢掉 —— 之前就因为这个漏掉了几条关键记录。
Start-Sleep -Seconds 2

$lines = Get-ChildItem $logDir -Filter *.log | ForEach-Object { Get-Content $_.FullName }

Stop-Process -Id $proc.Id -Force

Write-Host "`n===== 分段耗时（format=$format）====="

# 把 tracing 的 `key=value` 字段拉成一张表。三轮各一行，方便直接看冷热差异。
$fields = @(
  'capture_ms', 'load_ms', 'prepare_ms', 'show_ms', 'hotkey_to_visible_ms',
  'bytes', 'encode_ms', 'fetch_ms', 'decode_ms', 'hotkey_to_pixels_ms'
)

$round = 0
$current = $null
foreach ($line in $lines) {
  if ($line -match 'capture_ms=') {
    $round++
    $current = [ordered]@{ round = $round }
  }
  if (-not $current) { continue }

  foreach ($field in $fields) {
    if ($line -match "\b$field=([0-9.]+)") {
      $current[$field] = [double]$Matches[1]
    }
  }

  # 像素那条路是最后到的，拿它当一轮的收尾标记。
  if ($line -match 'hotkey_to_pixels_ms=') {
    [pscustomobject]$current | Format-List
    $current = $null
  }
}

Write-Host "`n===== 完整日志 ====="
$lines
