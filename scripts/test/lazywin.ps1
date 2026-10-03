# 长截图测试窗口：模拟视频站首页（不用浏览器，免得碰到浏览器账户和同步）。
#  - 顶部固定栏（不随滚动）
#  - 卡片网格，封面先是灰色占位，滚进视口后延迟 300–600ms 才"加载"出彩色封面
#  - 滚到底自动追加下一批卡片
#  - 右下角固定的"顶部"按钮（浮在滚动区上面）
# 物理坐标 (300,300) 起，1200×1300。卡片标题带编号，拼接结果可以逐张核对。
param([int]$Seconds = 600)
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class DpiL { [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v); }
"@
[void][DpiL]::SetProcessDpiAwarenessContext([IntPtr](-4))

$f = New-Object System.Windows.Forms.Form
$f.Text = "LazyTarget"; $f.StartPosition = 'Manual'; $f.Left = 300; $f.Top = 300; $f.Width = 1200; $f.Height = 1300; $f.TopMost = $true
$f.BackColor = [System.Drawing.Color]::FromArgb(246, 247, 248)

$header = New-Object System.Windows.Forms.Panel
$header.Dock = 'Top'; $header.Height = 90; $header.BackColor = [System.Drawing.Color]::White
$logo = New-Object System.Windows.Forms.Label
$logo.Text = "测试站   首页   动画   音乐   游戏   知识"; $logo.Font = New-Object System.Drawing.Font("Microsoft YaHei", 15)
$logo.ForeColor = [System.Drawing.Color]::FromArgb(251, 114, 153); $logo.AutoSize = $true; $logo.Left = 30; $logo.Top = 26
$header.Controls.Add($logo)

$flow = New-Object System.Windows.Forms.FlowLayoutPanel
$flow.Dock = 'Fill'; $flow.AutoScroll = $true; $flow.Padding = New-Object System.Windows.Forms.Padding(24, 16, 24, 16)

$top = New-Object System.Windows.Forms.Label
$top.Text = "顶部"; $top.TextAlign = 'MiddleCenter'; $top.Width = 70; $top.Height = 70; $top.BackColor = [System.Drawing.Color]::White
$top.Font = New-Object System.Drawing.Font("Microsoft YaHei", 11); $top.BorderStyle = 'FixedSingle'

$f.Controls.Add($flow); $f.Controls.Add($header); $f.Controls.Add($top)
$top.BringToFront()
$f.Add_Shown({ $top.Left = $f.ClientSize.Width - 110; $top.Top = $f.ClientSize.Height - 110 })

$words = "截图 识字 翻译 剪贴板 长截图 贴图 标注 马赛克 箭头 画笔 拼接 算法 模板 匹配 懒加载 封面 视频 弹幕".Split(' ')
$script:n = 0
$script:pending = New-Object System.Collections.ArrayList
$covers = New-Object System.Collections.ArrayList

function New-Cover([int]$i, [int]$w, [int]$h) {
  $bmp = New-Object System.Drawing.Bitmap $w, $h
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $c1 = [System.Drawing.Color]::FromArgb(255, (($i * 53) % 200) + 40, (($i * 97) % 200) + 30, (($i * 31) % 200) + 50)
  $c2 = [System.Drawing.Color]::FromArgb(255, (($i * 17) % 120) + 20, (($i * 71) % 150) + 40, (($i * 113) % 180) + 30)
  $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush((New-Object System.Drawing.Rectangle 0, 0, $w, $h), $c1, $c2, 35.0)
  $g.FillRectangle($brush, 0, 0, $w, $h)
  $g.DrawString("#$i", (New-Object System.Drawing.Font("Consolas", 20, [System.Drawing.FontStyle]::Bold)), [System.Drawing.Brushes]::White, $w - 90, $h - 44)
  $g.Dispose(); $bmp
}

function Add-Batch([int]$count) {
  $flow.SuspendLayout()
  for ($k = 0; $k -lt $count; $k++) {
    $script:n++
    $i = $script:n
    $card = New-Object System.Windows.Forms.Panel
    $card.Width = 260; $card.Height = 250; $card.Margin = New-Object System.Windows.Forms.Padding(8)
    $pic = New-Object System.Windows.Forms.PictureBox
    $pic.Width = 260; $pic.Height = 146; $pic.BackColor = [System.Drawing.Color]::FromArgb(227, 229, 231); $pic.Tag = $i
    $title = New-Object System.Windows.Forms.Label
    $title.Top = 152; $title.Width = 260; $title.Height = 56; $title.Font = New-Object System.Drawing.Font("Microsoft YaHei", 11)
    $title.Text = "第 $i 个视频 · " + (($words | Get-Random -Count (3 + $i % 3) -SetSeed $i) -join ' ')
    $meta = New-Object System.Windows.Forms.Label
    $meta.Top = 212; $meta.Width = 260; $meta.Height = 30; $meta.ForeColor = [System.Drawing.Color]::Gray
    $meta.Font = New-Object System.Drawing.Font("Microsoft YaHei", 10); $meta.Text = "UP 主 $($i % 13) · $(($i * 131) % 9000 + 100) 播放"
    $card.Controls.Add($pic); $card.Controls.Add($title); $card.Controls.Add($meta)
    $flow.Controls.Add($card)
    [void]$covers.Add($pic)
  }
  $flow.ResumeLayout()
}

# 每 100ms 检查一次：进了视口的占位封面排队，到时间就"加载好"
$tick = New-Object System.Windows.Forms.Timer; $tick.Interval = 100
$tick.Add_Tick({
  $now = [DateTime]::Now
  foreach ($pic in $covers) {
    if ($pic.Image -ne $null -or $script:pending.Contains($pic)) { continue }
    $pt = $flow.PointToClient($pic.PointToScreen([System.Drawing.Point]::Empty))
    if ($pt.Y -lt $flow.ClientSize.Height -and $pt.Y + $pic.Height -gt 0) {
      $pic.Name = $now.AddMilliseconds(300 + ([int]$pic.Tag * 37) % 300).Ticks.ToString()
      [void]$script:pending.Add($pic)
    }
  }
  foreach ($pic in @($script:pending)) {
    if ([long]$pic.Name -le $now.Ticks) { $pic.Image = New-Cover ([int]$pic.Tag) $pic.Width $pic.Height; $script:pending.Remove($pic) }
  }
  # 滚到底：过 700ms 再追加一批（模拟网络请求）
  if ($script:n -lt 72 -and -not $script:loadAt -and $flow.VerticalScroll.Value + $flow.ClientSize.Height -ge $flow.VerticalScroll.Maximum - 200) {
    $script:loadAt = $now.AddMilliseconds(700)
  }
  if ($script:loadAt -and $now -ge $script:loadAt) { $script:loadAt = $null; Add-Batch 12 }
})
Add-Batch 16
$tick.Start()
$t = New-Object System.Windows.Forms.Timer; $t.Interval = $Seconds * 1000; $t.Add_Tick({ $f.Close() }); $t.Start()
[void]$f.ShowDialog()
