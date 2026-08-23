# 直接量底图窗口的实际扩展样式和 z 序，不靠推断。
#
#   .\scripts\probe-zorder.ps1 <exe 路径> [nostyle]
#
# 回答两个问题：
#   1. `DeferWindowPos(hWndInsertAfter = 遮罩)` 会不会把底图带进 topmost 波段？
#      （`SetWindowPos` 文档说会，但文档和实现对不上的事不是没有过）
#   2. 底图和遮罩在 z 序里到底谁在谁上面？
#
# 传 nostyle 就设 CHENOCR_BACKDROP_NO_TOPMOST=1，也就是创建时**不**加
# WS_EX_TOPMOST —— 用来判断那个样式到底是必需的还是冗余的。
#
# 不属于产品代码。

$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\lib.ps1"

$exe = $args[0]
if (-not $exe) { throw 'usage: probe-zorder.ps1 <exe> [nostyle]' }
$noStyle = ($args.Count -ge 2 -and $args[1] -eq 'nostyle')

Reset-Chenocr

# 用 EnumWindows 按 pid 找，不用 FindWindowW：后者在这里返回 0，而按 pid 枚举
# 稳定能找到。顺带还能确认找到的窗口确实属于本次启动的进程。
Add-Type -Namespace Win32 -Name Zo -MemberDefinition @'
public delegate bool Proc(System.IntPtr h, System.IntPtr l);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern void keybd_event(byte vk, byte scan, uint flags, System.IntPtr extra);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool EnumWindows(Proc cb, System.IntPtr l);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern uint GetWindowThreadProcessId(System.IntPtr h, out uint pid);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern int GetWindowLongW(System.IntPtr hWnd, int index);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern System.IntPtr GetWindow(System.IntPtr hWnd, uint cmd);
[System.Runtime.InteropServices.DllImport("user32.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode)]
public static extern int GetClassNameW(System.IntPtr hWnd, System.Text.StringBuilder buf, int max);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool IsWindowVisible(System.IntPtr hWnd);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool SetProcessDpiAwarenessContext(System.IntPtr context);
'@

$null = [Win32.Zo]::SetProcessDpiAwarenessContext([IntPtr](-4))

$GWL_EXSTYLE   = -20
$WS_EX_TOPMOST = 0x00000008
$GW_HWNDPREV   = 3

Set-ChenocrEnv $true
if ($noStyle) { $env:CHENOCR_BACKDROP_NO_TOPMOST = '1' } else { Remove-Item Env:\CHENOCR_BACKDROP_NO_TOPMOST -ErrorAction SilentlyContinue }

$proc = Start-Process -FilePath $exe -PassThru
Start-Sleep -Seconds 6
Assert-Hotkey (Join-Path $env:APPDATA 'CHENOCR\logs')

# F1 让底图真正上屏 —— 样式是创建时定的，但 z 序只有显示之后才有意义
[Win32.Zo]::keybd_event(0x70, 0, 0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 40
[Win32.Zo]::keybd_event(0x70, 0, 2, [IntPtr]::Zero)
Start-Sleep -Milliseconds 1500

$backdrop = [IntPtr]::Zero
$owned = [System.Collections.ArrayList]::new()
$scan = [Win32.Zo+Proc]{ param($h, $l)
  $wpid = 0
  [void][Win32.Zo]::GetWindowThreadProcessId($h, [ref]$wpid)
  if ($wpid -eq $proc.Id) {
    $sb = New-Object System.Text.StringBuilder 256
    [void][Win32.Zo]::GetClassNameW($h, $sb, 256)
    $cls = $sb.ToString()
    $cex = [Win32.Zo]::GetWindowLongW($h, $GWL_EXSTYLE)
    [void]$owned.Add("    $cls  ex=0x$('{0:X8}' -f $cex)  topmost=$((($cex -band $WS_EX_TOPMOST) -ne 0))")
    if ($cls -eq 'ChenocrBackdrop') { $script:backdrop = $h }
  }
  return $true
}
[void][Win32.Zo]::EnumWindows($scan, [IntPtr]::Zero)

if ($backdrop -eq [IntPtr]::Zero) {
  [Win32.Zo]::keybd_event(0x1B, 0, 0, [IntPtr]::Zero)
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
  throw '没找到底图窗口（类名 ChenocrBackdrop）'
}

$ex = [Win32.Zo]::GetWindowLongW($backdrop, $GWL_EXSTYLE)
$isTopmost = ($ex -band $WS_EX_TOPMOST) -ne 0

# 往上走一路看谁在底图之上，直到 z 序顶端
$chain = [System.Collections.ArrayList]::new()
$cur = $backdrop
for ($i = 0; $i -lt 12; $i++) {
  $cur = [Win32.Zo]::GetWindow($cur, $GW_HWNDPREV)
  if ($cur -eq [IntPtr]::Zero) { break }
  if (-not [Win32.Zo]::IsWindowVisible($cur)) { continue }
  $sb = New-Object System.Text.StringBuilder 256
  $null = [Win32.Zo]::GetClassNameW($cur, $sb, $sb.Capacity)
  $cex = [Win32.Zo]::GetWindowLongW($cur, $GWL_EXSTYLE)
  $tm = if (($cex -band $WS_EX_TOPMOST) -ne 0) { 'TOPMOST' } else { '普通    ' }
  $null = $chain.Add("    $tm  $($sb.ToString())")
}

[Win32.Zo]::keybd_event(0x1B, 0, 0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 40
[Win32.Zo]::keybd_event(0x1B, 0, 2, [IntPtr]::Zero)
Start-Sleep -Milliseconds 500
Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
Remove-Item Env:\CHENOCR_BACKDROP_NO_TOPMOST -ErrorAction SilentlyContinue

$mode = if ($noStyle) { '创建时不加 WS_EX_TOPMOST' } else { '创建时加了 WS_EX_TOPMOST' }
Write-Host ""
Write-Host "===== 底图窗口 z 序实测 ====="
Write-Host "模式         : $mode"
Write-Host "GWL_EXSTYLE  : 0x$('{0:X8}' -f $ex)"
Write-Host "实际 TOPMOST : $isTopmost"
Write-Host "底图之上依次是（从近到远）:"
if ($chain.Count -eq 0) { Write-Host '    （无，底图就在 z 序顶端）' } else { $chain | ForEach-Object { Write-Host $_ } }
Write-Host "本进程的全部顶层窗口:"
$owned | ForEach-Object { Write-Host $_ }
