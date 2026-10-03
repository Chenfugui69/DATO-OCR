# 粘贴测试用的富文本窗口：物理坐标 (300,300) 起，900×500。每 300ms 把状态写到同目录 paste-out.txt：
# 文本、是否贴进了图片、收到几次 Ctrl+V、收到 Ctrl+V 时剪贴板里有哪些格式。
param([int]$Seconds = 600)
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class DpiP { [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v); }
"@
[void][DpiP]::SetProcessDpiAwarenessContext([IntPtr](-4))
$out = Join-Path $PSScriptRoot 'paste-out.txt'
Set-Content -Path $out -Value '' -NoNewline -Encoding utf8
$f = New-Object System.Windows.Forms.Form
$f.Text = "PasteTarget"; $f.StartPosition = 'Manual'; $f.Left = 300; $f.Top = 300; $f.Width = 900; $f.Height = 500; $f.TopMost = $true
$tb = New-Object System.Windows.Forms.RichTextBox
$tb.Dock = 'Fill'; $tb.Font = New-Object System.Drawing.Font("Microsoft YaHei", 14)
$script:gotV = 0; $script:fmts = ""
$tb.Add_KeyDown({ if ($_.Control -and $_.KeyCode -eq 'V') { $script:gotV++; $script:fmts = [System.Windows.Forms.Clipboard]::GetDataObject().GetFormats() -join ',' } })
$f.Controls.Add($tb)
$w = New-Object System.Windows.Forms.Timer; $w.Interval = 300; $w.Add_Tick({ $pict = $tb.Rtf.Contains("\pict"); Set-Content -Path $out -Value ("text=" + $tb.Text + "|pict=" + $pict + "|rtflen=" + $tb.Rtf.Length + "|ctrlV=" + $script:gotV + "|fmts=" + $script:fmts + "|focused=" + $tb.Focused) -NoNewline -Encoding utf8 }); $w.Start()
$t = New-Object System.Windows.Forms.Timer; $t.Interval = $Seconds * 1000; $t.Add_Tick({ $f.Close() }); $t.Start()
[void]$f.ShowDialog()


