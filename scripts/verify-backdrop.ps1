# M0 验收：底图是不是「冻结的、与真实桌面像素级一致」的画面。
#
#   .\scripts\verify-backdrop.ps1 <exe 路径>
#
# 这是规格 08 里 M0 那条"背景像素级一致"的自动化版本。做法：
#
#   1. 截一块屏幕区域，存为 A
#   2. 按 F1 冻结画面
#   3. **改变真实桌面**（弹一个记事本出来）
#   4. 在同一块区域拖出选区 —— 选区内部是不压暗的原始底图
#   5. 再截同一块区域，存为 B
#   6. 逐像素比 A 和 B
#
# A == B 同时证明三件事：
#   - 底图真的盖住了真实桌面（否则第 3 步的记事本会出现在 B 里）
#   - 底图是冻结的（同上）
#   - 底图 1:1 落在物理像素上，没有重采样、没有偏移、没有色彩管理改动过像素值
#
# 必须开 CHENOCR_ALLOW_SELF_CAPTURE，否则截到的永远是真实桌面（见
# platform/windows/self_capture.rs）。
#
# 不属于产品代码。

$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\lib.ps1"

$exe = $args[0]
if (-not $exe) { throw 'usage: verify-backdrop.ps1 <exe>' }

Reset-Chenocr

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type -Namespace Win32 -Name Vb -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern void keybd_event(byte vk, byte scan, uint flags, System.IntPtr extra);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool SetCursorPos(int x, int y);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, System.IntPtr extra);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool SetProcessDpiAwarenessContext(System.IntPtr context);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool SetWindowPos(System.IntPtr hWnd, System.IntPtr after, int x, int y, int cx, int cy, uint flags);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool BringWindowToTop(System.IntPtr hWnd);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool SetForegroundWindow(System.IntPtr hWnd);
'@

# 脚本自己必须是 Per-Monitor-V2，否则所有坐标都会被系统虚拟化，选区尺寸对不上。
$null = [Win32.Vb]::SetProcessDpiAwarenessContext([IntPtr](-4))

function Send-Key([byte]$vk) {
  [Win32.Vb]::keybd_event($vk, 0, 0, [IntPtr]::Zero)
  Start-Sleep -Milliseconds 40
  [Win32.Vb]::keybd_event($vk, 0, 2, [IntPtr]::Zero)
}

# 选区：拖 (X1,Y1) → (X2,Y2)。比较时四边各留 8px，避开选区边框和尺寸标签。
$X1 = 700; $Y1 = 500; $X2 = 1660; $Y2 = 1040
$INSET = 8

function Grab([int]$x, [int]$y, [int]$w, [int]$h) {
  $bmp = New-Object System.Drawing.Bitmap($w, $h)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
  $g.Dispose()
  return $bmp
}

Set-ChenocrEnv $true
$proc = Start-Process -FilePath $exe -PassThru
Write-Host "已启动 pid=$($proc.Id)，等待托盘就绪..."
Start-Sleep -Seconds 6
Assert-Hotkey (Join-Path $env:APPDATA 'CHENOCR\logs')

$cx = $X1 + $INSET
$cy = $Y1 + $INSET
$cw = ($X2 - $X1) - 2 * $INSET
$ch = ($Y2 - $Y1) - 2 * $INSET

# 入侵窗口自己用 WinForms 建，不用 cmd / notepad：
#   - Win11 的 cmd 由 Windows Terminal 托管、notepad 是 Store 应用，两者的
#     `MainWindowHandle` 都是 0，拿不到句柄就没法让它主动抢 z 序，测试退化成
#     "看运气"（M0 期间真的一次过一次不过）
#   - 自己建的窗口可以精确铺在比对区域上，并且用洋红这种桌面上绝不会出现的颜色
#     —— 比对失败时 B 的像素值直接告出是谁露出来的
# 停车位：远离比对区，屏幕右下角。入侵窗口先停在这里，等冻结之后再挪进比对区。
$PARK_X = 2400; $PARK_Y = 1400

$intruder = New-Object System.Windows.Forms.Form
$intruder.FormBorderStyle = 'None'
$intruder.BackColor = [System.Drawing.Color]::Magenta
$intruder.ShowInTaskbar = $false
$intruder.StartPosition = 'Manual'
$intruder.Bounds = New-Object System.Drawing.Rectangle($PARK_X, $PARK_Y, $cw, $ch)

$SWP_KEEP_Z = 0x0001 -bor 0x0004 -bor 0x0010   # NOSIZE|NOZORDER|NOACTIVATE
$SWP_RAISE  = 0x0001 -bor 0x0002 -bor 0x0010 -bor 0x0040
$HWND_TOPMOST = [IntPtr](-1)
$MAGENTA = [System.Drawing.Color]::Magenta

# 入侵窗口必须是 **topmost**，而且必须在按 F1 **之前**就存在。
#
# z 序分两个波段，topmost 的一律在非 topmost 的之上：
#
#   - 用普通窗口测没有意义：它永远在下面那个波段，压根到不了遮罩下方那个夹层，
#     所以不管底图有没有 WS_EX_TOPMOST 都会「通过」。
#   - 冻结之后才把窗口摆到 HWND_TOPMOST 也没有意义：那会把它插到 topmost 波段的
#     **最顶上**，连遮罩一起盖住。任何截图工具都拦不住这种情况，测它等于测系统。
#
# 有意义的是这个：置顶窗口先存在，然后遮罩被抬到 topmost 波段顶部（盖住它），
# 底图紧贴遮罩之下。此时
#   - 底图也在 topmost 波段 → 入侵窗口在底图**之下**，被挡住，看不见
#   - 底图掉到普通波段     → 入侵窗口卡在遮罩和底图**之间**，透过镂空的选区可见，
#                            而且是活动内容浮在冻结画面上
#
# 现实对应物：音量 OSD、画中画、Teams 悬浮条、PowerToys 置顶窗、录屏工具条、
# 输入法候选窗、任务管理器的"置于顶层"。
function Show-Intruder {
  $intruder.Show()
  $null = [Win32.Vb]::SetWindowPos($intruder.Handle, $HWND_TOPMOST, 0, 0, 0, 0, $SWP_RAISE)
  foreach ($t in 1..10) { [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 60 }
}

# 挪进比对区，但**不动 z 序**（SWP_NOZORDER）。动了 z 序就会把它重新插到波段顶部，
# 又变成上面说的那种「测系统」的无意义情况。
function Move-IntruderIntoRegion {
  $null = [Win32.Vb]::SetWindowPos($intruder.Handle, [IntPtr]0, $cx, $cy, 0, 0, $SWP_KEEP_Z)
  foreach ($t in 1..10) { [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 60 }
}

function Count-Magenta($bmp) {
  # 抽样就够，这里只是要个"有没有"的定性判断，不需要逐像素
  $hit = 0
  for ($y = 0; $y -lt $bmp.Height; $y += 16) {
    for ($x = 0; $x -lt $bmp.Width; $x += 16) {
      $p = $bmp.GetPixel($x, $y)
      if ($p.R -eq $MAGENTA.R -and $p.G -eq $MAGENTA.G -and $p.B -eq $MAGENTA.B) { $hit++ }
    }
  }
  return $hit
}

# ── 对照组：先证明这个入侵窗口是真的能被抓到的 ──────────────────────────────
# 少了这一步，"B 里没有洋红"有两种解释：底图挡住了，或者窗口根本没画出来。
# 后者会让测试假过 —— 比失败更糟。
Show-Intruder
$control = Grab $PARK_X $PARK_Y $cw $ch
$controlHits = Count-Magenta $control
$control.Dispose()
if ($controlHits -eq 0) {
  $intruder.Close(); $intruder.Dispose()
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
  throw '对照组失败：入侵窗口没被抓到，这一轮什么都证明不了'
}
# 注意：**不隐藏**。它要作为"按 F1 之前就已存在的置顶窗口"留在原地（停车位上，
# 不在比对区内），这样遮罩上屏时才会被抬到它上面。

# ── A：干净桌面 ────────────────────────────────────────────────────────────
#
# 从这里到 F1 之间**绝对不能让屏幕发生任何变化**，否则冻结下来的是变化之后的画面，
# 和 A 天然不同，比对必然失败 —— 而失败原因是测试自己造的，不是产品的问题。
#
# 这正是 M0 期间那次"间歇性失败"的真凶：原来在 A 和 F1 之间有一句 Write-Host，
# 它把终端滚了一行，而比对区恰好压在终端上。所以：
#   - 所有输出攒到比对结束后再打（$log）
#   - 抓 A 之前先静默等一段，让之前的输出渲染完、动画停下来
$log = [System.Collections.ArrayList]::new()
$null = $log.Add("对照组: 无遮罩时抓到洋红采样点 $controlHits 个")

Start-Sleep -Milliseconds 1200                   # 静默沉降期，不许有输出

$before = Grab $cx $cy $cw $ch
$null = $log.Add("A: 已截取 ${cw}x${ch} @ ($cx,$cy)")

Send-Key 0x70                                    # F1，冻结
Start-Sleep -Milliseconds 1500

# 改变真实桌面：把那个**冻结前就存在的置顶窗口**挪进比对区。
# 底图既然是冻结的、且和遮罩同在 topmost 波段，洋红就绝不该出现在 B 里。
Move-IntruderIntoRegion
$null = $log.Add("置顶入侵窗口已挪进比对区（未动 z 序）hwnd=$($intruder.Handle)")

$null = [Win32.Vb]::SetCursorPos($X1, $Y1)
Start-Sleep -Milliseconds 150
[Win32.Vb]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)   # LEFTDOWN
foreach ($step in 1..10) {
  $null = [Win32.Vb]::SetCursorPos(
    $X1 + [int](($X2 - $X1) * $step / 10),
    $Y1 + [int](($Y2 - $Y1) * $step / 10))
  # 入侵窗口得继续泵消息，否则它不重绘，洋红可能根本没画上去，
  # 测试会假过 —— 那比失败更糟。
  [System.Windows.Forms.Application]::DoEvents()
  Start-Sleep -Milliseconds 30
}
[Win32.Vb]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)   # LEFTUP
foreach ($t in 1..8) { [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 60 }

$after = Grab $cx $cy $cw $ch
$afterHits = Count-Magenta $after
$null = $log.Add("B: 已截取同一区域，洋红采样点 $afterHits 个")

Send-Key 0x1B                                    # Esc
Start-Sleep -Milliseconds 500
# 清理失败不该盖掉比对结果 —— 那才是这个脚本的产出。
$intruder.Close()
$intruder.Dispose()
Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue

# ── 逐像素比 ──────────────────────────────────────────────────────────────
$diff = 0
$maxDelta = 0
$firstAt = $null

for ($y = 0; $y -lt $ch; $y++) {
  for ($x = 0; $x -lt $cw; $x++) {
    $a = $before.GetPixel($x, $y)
    $b = $after.GetPixel($x, $y)
    if ($a.R -ne $b.R -or $a.G -ne $b.G -or $a.B -ne $b.B) {
      $diff++
      if (-not $firstAt) { $firstAt = "($($x + $cx),$($y + $cy)) A=$($a.R),$($a.G),$($a.B) B=$($b.R),$($b.G),$($b.B)" }
      $d = [Math]::Max([Math]::Abs($a.R - $b.R), [Math]::Max([Math]::Abs($a.G - $b.G), [Math]::Abs($a.B - $b.B)))
      if ($d -gt $maxDelta) { $maxDelta = $d }
    }
  }
}

$total = $cw * $ch
$pct = [Math]::Round(100.0 * $diff / $total, 4)

$log | ForEach-Object { Write-Host $_ }
Write-Host ""
Write-Host "===== 像素级一致性 ====="
Write-Host "比较像素   : $total"
Write-Host "不一致     : $diff  ($pct%)"
Write-Host "一致       : $($total - $diff)"
Write-Host "最大通道差 : $maxDelta"
if ($firstAt) { Write-Host "首个差异   : $firstAt" }
if ($afterHits -gt 0) {
  Write-Host "洋红采样点 : $afterHits  ← 入侵窗口从选区里露出来了，底图没挡住"
}

if ($diff -ne 0) {
  # 失败时把两张图落盘。差异率和首个差异点说明不了"到底看到了什么"，
  # 而肉眼看一眼 B 一般当场就能定性：是没冻住、是压暗了、还是错位。
  $before.Save("$PSScriptRoot\backdrop-A.png", [System.Drawing.Imaging.ImageFormat]::Png)
  $after.Save("$PSScriptRoot\backdrop-B.png", [System.Drawing.Imaging.ImageFormat]::Png)
  Write-Host "已存图     : scripts\backdrop-A.png（真实桌面）scripts\backdrop-B.png（选区内底图）"
}

$before.Dispose()
$after.Dispose()

if ($diff -eq 0) {
  Write-Host "`n通过：底图与真实桌面逐字节一致，且确实是冻结画面。"
  exit 0
}
Write-Host "`n失败：底图与真实桌面不一致。"
exit 1
