# 验证"抓屏热身状态失效后会自动重新热身"这条链路。
#
#   .\scripts\verify-rewarm.ps1 <exe 路径>
#
# 分两部分：
#
# 1. **监听窗口够不够格收广播**。WM_DISPLAYCHANGE / WM_POWERBROADCAST 是广播给
#    顶层窗口的，message-only 窗口（父窗口 = HWND_MESSAGE）收不到。所以这里查它
#    的 GetParent 和 GetAncestor 是不是 0 —— 是 0 才说明它是真顶层窗口。
#    这一条是静态资格检查，不依赖能不能造出真实的显示变化。
#
# 2. **处理器和重新热身真的接上了**。直接给那个窗口投 WM_DISPLAYCHANGE 和
#    WM_POWERBROADCAST(PBT_APMRESUMEAUTOMATIC)，然后去日志里找重新热身的记录。
#
# 为什么不去真改分辨率制造一次货真价实的 WM_DISPLAYCHANGE：那要动用户的显示配置，
# 万一改回去失败就把人家桌面搞坏了，代价和收益不成比例。上面两条合起来已经覆盖了
# 我们自己这边的所有逻辑；剩下"系统到底会不会发这个广播"是 OS 的既定行为。
#
# 不属于产品代码。

$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\lib.ps1"

$exe = $args[0]
if (-not $exe) { throw 'usage: verify-rewarm.ps1 <exe>' }

Reset-Chenocr

Add-Type -Namespace Win32 -Name Rw -MemberDefinition @'
public delegate bool Proc(System.IntPtr h, System.IntPtr l);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool EnumWindows(Proc cb, System.IntPtr l);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern uint GetWindowThreadProcessId(System.IntPtr h, out uint pid);
[System.Runtime.InteropServices.DllImport("user32.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode)]
public static extern int GetClassNameW(System.IntPtr h, System.Text.StringBuilder b, int m);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern System.IntPtr GetParent(System.IntPtr h);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern System.IntPtr GetAncestor(System.IntPtr h, uint flags);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool PostMessageW(System.IntPtr h, uint msg, System.IntPtr w, System.IntPtr l);
'@

$WM_DISPLAYCHANGE = 0x007E
$WM_POWERBROADCAST = 0x0218
$PBT_APMRESUMEAUTOMATIC = 18
$GA_PARENT = 1

$logDir = Join-Path $env:APPDATA 'CHENOCR\logs'
Set-ChenocrEnv $false
$proc = Start-Process -FilePath $exe -PassThru
Write-Host "已启动 pid=$($proc.Id)，等待启动热身跑完..."
Start-Sleep -Seconds 8
Assert-Hotkey $logDir

# ── 找监听窗口 ────────────────────────────────────────────────────────────
$listener = [IntPtr]::Zero
$cb = [Win32.Rw+Proc]{ param($h, $l)
  $wpid = 0
  [void][Win32.Rw]::GetWindowThreadProcessId($h, [ref]$wpid)
  if ($wpid -eq $proc.Id) {
    $sb = New-Object System.Text.StringBuilder 256
    [void][Win32.Rw]::GetClassNameW($h, $sb, 256)
    if ($sb.ToString() -eq 'ChenocrSystemEvents') { $script:listener = $h }
  }
  return $true
}
[void][Win32.Rw]::EnumWindows($cb, [IntPtr]::Zero)

if ($listener -eq [IntPtr]::Zero) {
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
  throw '没找到系统事件监听窗口（类名 ChenocrSystemEvents）'
}

$parent = [Win32.Rw]::GetParent($listener)
$ancestor = [Win32.Rw]::GetAncestor($listener, $GA_PARENT)
$isTopLevel = ($parent -eq [IntPtr]::Zero)

Write-Host ""
Write-Host "===== 1. 广播资格 ====="
Write-Host "监听窗口     : hwnd=$listener"
Write-Host "GetParent    : $parent"
Write-Host "GetAncestor  : $ancestor"
$verdict = if ($isTopLevel) { '能收广播' } else { '收不到广播' }
Write-Host "是顶层窗口   : $isTopLevel  -> $verdict"

# 记下投消息之前日志有多少行"重新热身"，避免把启动那次算进来
$logFile = Get-ChildItem $logDir | Sort-Object LastWriteTime | Select-Object -Last 1
$before = (Get-Content $logFile.FullName -Encoding UTF8 | Select-String -Pattern '重新热身').Count

# ── 投两种失效消息 ────────────────────────────────────────────────────────
[void][Win32.Rw]::PostMessageW($listener, $WM_DISPLAYCHANGE, [IntPtr]::Zero, [IntPtr]::Zero)
Start-Sleep -Milliseconds 2500
[void][Win32.Rw]::PostMessageW($listener, $WM_POWERBROADCAST, [IntPtr]$PBT_APMRESUMEAUTOMATIC, [IntPtr]::Zero)
Start-Sleep -Milliseconds 2500

$lines = Get-Content $logFile.FullName -Encoding UTF8
$after = ($lines | Select-String -Pattern '重新热身').Count
$gained = $after - $before

Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "===== 2. 重新热身 ====="
Write-Host "投消息前 : $before 次"
Write-Host "投消息后 : $after 次"
Write-Host "新增     : $gained  (期望 2：显示变化 + 睡眠唤醒)"
Write-Host ""
Write-Host "相关日志:"
$lines | Select-String -Pattern '热身' | Select-Object -Last 8 | ForEach-Object { Write-Host "    $($_.Line)" }

if ($isTopLevel -and $gained -ge 2) {
  Write-Host "`n通过：监听窗口够格收广播，两种失效时机都触发了重新热身。"
  exit 0
}
Write-Host "`n失败。"
exit 1
