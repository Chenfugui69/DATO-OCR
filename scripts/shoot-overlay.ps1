# 把「截图界面自己长什么样」拍下来，用于肉眼验收。
#
#   .\scripts\shoot-overlay.ps1 <exe 路径> [输出 png]
#
# 按 F1 打开遮罩，拖一个选区出来，然后截整屏存盘，最后 Esc 退出。
#
# 必须开 CHENOCR_ALLOW_SELF_CAPTURE —— 遮罩窗口和底图窗口平时都设了
# WDA_EXCLUDEFROMCAPTURE，那是 DWM 层面强制的，不关掉的话截出来只有真实桌面，
# 看不到我们画的任何东西。
#
# 不属于产品代码。

$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\lib.ps1"

$exe = $args[0]
$out = if ($args.Count -ge 2) { $args[1] } else { 'scripts/overlay-shot.png' }
if (-not $exe) { throw 'usage: shoot-overlay.ps1 <exe> [out.png]' }

Reset-Chenocr

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type -Namespace Win32 -Name Shot -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern void keybd_event(byte vk, byte scan, uint flags, System.IntPtr extra);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool SetCursorPos(int x, int y);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, System.IntPtr extra);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool SetProcessDpiAwarenessContext(System.IntPtr context);
'@

# 脚本自己也得是 Per-Monitor-V2，否则 SetCursorPos 收到的是被虚拟化过的坐标，
# 拖出来的选区尺寸会按缩放比例放大。
$null = [Win32.Shot]::SetProcessDpiAwarenessContext([IntPtr](-4))

function Send-Key([byte]$vk) {
  [Win32.Shot]::keybd_event($vk, 0, 0, [IntPtr]::Zero)
  Start-Sleep -Milliseconds 40
  [Win32.Shot]::keybd_event($vk, 0, 2, [IntPtr]::Zero)
}

Set-ChenocrEnv $true
$proc = Start-Process -FilePath $exe -PassThru
Write-Host "已启动 pid=$($proc.Id)，等待托盘就绪..."
Start-Sleep -Seconds 6
Assert-Hotkey (Join-Path $env:APPDATA 'CHENOCR\logs')

Send-Key 0x70                                    # F1
Start-Sleep -Milliseconds 1200

# 拖一个选区，好把压暗层、镂空和尺寸标签都拍进去
$null = [Win32.Shot]::SetCursorPos(700, 500)
Start-Sleep -Milliseconds 120
[Win32.Shot]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)   # LEFTDOWN
foreach ($step in 1..12) {
  $null = [Win32.Shot]::SetCursorPos(700 + $step * 80, 500 + $step * 45)
  Start-Sleep -Milliseconds 25
}
[Win32.Shot]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)   # LEFTUP
Start-Sleep -Milliseconds 400

$bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bitmap = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($bounds.X, $bounds.Y, 0, 0, $bitmap.Size)

$path = Join-Path (Get-Location) $out
$bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bitmap.Dispose()

Write-Host "已存: $path  ($($bounds.Width)x$($bounds.Height))"

Send-Key 0x1B                                    # Esc
Start-Sleep -Milliseconds 800
Stop-Process -Id $proc.Id -Force
