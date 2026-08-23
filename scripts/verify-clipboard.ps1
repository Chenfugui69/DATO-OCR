# M0 验收用的一次性脚本：合成一次"F1 → 拖拽框选 → Enter"，再从系统剪贴板
# 读回图片，核对尺寸是否和选区严格一致（验收清单里的"尺寸精确"）。
# 不属于产品代码，验完就可以删。

$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\lib.ps1"

$exe = $args[0]
$logDir = Join-Path $env:APPDATA 'CHENOCR\logs'

Reset-Chenocr

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

Add-Type -Namespace Win32 -Name Sim -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern void keybd_event(byte vk, byte scan, uint flags, System.IntPtr extra);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool SetCursorPos(int x, int y);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, System.IntPtr extra);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool SetProcessDpiAwarenessContext(System.IntPtr context);
'@

# PowerShell 默认是 DPI-unaware 的，那样 SetCursorPos 收的是**虚拟化后**的逻辑
# 坐标，Windows 会替我们乘一个缩放比。在 150% 的屏上就意味着"我以为点了 600，
# 实际落在 900"，对不上 Rust 侧的物理坐标，核对尺寸时会白白差一个 1.5 倍。
# 声明 Per-Monitor-V2（-4）之后，下面所有坐标才是真正的物理像素。
$DPI_AWARENESS_PER_MONITOR_V2 = [IntPtr](-4)
$dpiOk = [Win32.Sim]::SetProcessDpiAwarenessContext($DPI_AWARENESS_PER_MONITOR_V2)
if (-not $dpiOk) { Write-Host "WARN: SetProcessDpiAwarenessContext failed, coords may be virtualized" }

function Send-Key([byte]$vk) {
  [Win32.Sim]::keybd_event($vk, 0, 0, [IntPtr]::Zero)
  Start-Sleep -Milliseconds 40
  [Win32.Sim]::keybd_event($vk, 0, 2, [IntPtr]::Zero)
}

$MOUSE_LEFTDOWN = 0x0002
$MOUSE_LEFTUP = 0x0004

# 选区用物理像素表示。SetCursorPos 收的就是物理屏幕坐标，
# 和 Rust 侧存的虚拟桌面物理坐标是同一套，方便直接对账。
$x1, $y1 = 600, 400
$x2, $y2 = 1000, 700
$expectW = $x2 - $x1
$expectH = $y2 - $y1

if (Test-Path $logDir) { Remove-Item $logDir -Recurse -Force }
[System.Windows.Forms.Clipboard]::Clear()

Set-ChenocrEnv $false
$proc = Start-Process -FilePath $exe -PassThru
Write-Host "已启动 pid=$($proc.Id)，等待预建遮罩..."
Start-Sleep -Seconds 6
Assert-Hotkey $logDir

Write-Host "F1 → 从 ($x1,$y1) 拖到 ($x2,$y2) → Enter"
Send-Key 0x70                       # VK_F1
Start-Sleep -Milliseconds 1500      # 等遮罩上屏

[Win32.Sim]::SetCursorPos($x1, $y1) | Out-Null
Start-Sleep -Milliseconds 150
[Win32.Sim]::mouse_event($MOUSE_LEFTDOWN, 0, 0, 0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 100

# 分几步移动，模拟真实拖拽（一步到位可能被当成点击）
foreach ($i in 1..8) {
  $x = $x1 + [int](($x2 - $x1) * $i / 8)
  $y = $y1 + [int](($y2 - $y1) * $i / 8)
  [Win32.Sim]::SetCursorPos($x, $y) | Out-Null
  Start-Sleep -Milliseconds 40
}

[Win32.Sim]::mouse_event($MOUSE_LEFTUP, 0, 0, 0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 300
Send-Key 0x0D                       # VK_RETURN
Start-Sleep -Seconds 2

Write-Host "`n===== 剪贴板 ====="
if ([System.Windows.Forms.Clipboard]::ContainsImage()) {
  $img = [System.Windows.Forms.Clipboard]::GetImage()
  Write-Host ("剪贴板图片尺寸 {0}x{1}，期望 {2}x{3}" -f $img.Width, $img.Height, $expectW, $expectH)
  if ($img.Width -eq $expectW -and $img.Height -eq $expectH) {
    Write-Host "尺寸一致 PASS"
  } else {
    Write-Host "尺寸不一致 FAIL"
  }

  # 顺手看一眼像素是不是不透明（alpha 修复的回归检查）
  $c = $img.GetPixel([int]($img.Width / 2), [int]($img.Height / 2))
  Write-Host ("中心像素 A={0} R={1} G={2} B={3}" -f $c.A, $c.R, $c.G, $c.B)

  $out = Join-Path $PSScriptRoot 'clipboard-check.png'
  $img.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
  Write-Host "已存盘：$out"
} else {
  Write-Host "剪贴板里没有图片 FAIL"
}

Write-Host "`n===== 日志 ====="
Get-ChildItem $logDir -Filter *.log | ForEach-Object { Get-Content $_.FullName }

Stop-Process -Id $proc.Id -Force

# 残留进程检查（验收项："无残留进程"）
Start-Sleep -Milliseconds 800
$left = Get-Process -Name chenocr -ErrorAction SilentlyContinue
if ($left) { Write-Host "`n仍有 chenocr 进程残留 FAIL" } else { Write-Host "`n无残留进程 PASS" }
