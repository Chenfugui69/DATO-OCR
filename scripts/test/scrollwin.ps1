# 长截图测试用的可滚动窗口：DPI 感知，物理坐标 (300,300) 起，1100×1300，内容每行不同。
param([int]$Seconds = 60)
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class DpiS { [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v); }
"@
[void][DpiS]::SetProcessDpiAwarenessContext([IntPtr](-4))
$f = New-Object System.Windows.Forms.Form
$f.Text = "ScrollTarget"; $f.StartPosition = 'Manual'; $f.Left = 300; $f.Top = 300; $f.Width = 1100; $f.Height = 1300; $f.TopMost = $true
$tb = New-Object System.Windows.Forms.RichTextBox
$tb.Dock = 'Fill'; $tb.Font = New-Object System.Drawing.Font("Microsoft YaHei", 14); $tb.ReadOnly = $true; $tb.BorderStyle = 'None'
$words = "截图 识字 翻译 剪贴板 长截图 贴图 标注 马赛克 箭头 画笔 DATOCOR Tauri React Rust 拼接 算法 模板 匹配 头部 尾部".Split(' ')
$lines = for ($i = 1; $i -le 400; $i++) { $n = ($i * 7) % 6 + 3; "第 {0:D3} 行  " -f $i + (($words | Get-Random -Count $n -SetSeed $i) -join ' ') }
$tb.Text = $lines -join "`n"
$f.Controls.Add($tb)
$t = New-Object System.Windows.Forms.Timer; $t.Interval = $Seconds * 1000; $t.Add_Tick({ $f.Close() }); $t.Start()
[void]$f.ShowDialog()
