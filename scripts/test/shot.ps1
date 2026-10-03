# 截整屏或指定区域（屏幕物理像素），可缩放后存 PNG。
# 用法：shot.ps1 -Out a.png [-X 0 -Y 0 -W 800 -H 600] [-Scale 0.5]
param(
  [string]$Out = "$PSScriptRoot\screen.png",
  [int]$X = 0, [int]$Y = 0, [int]$W = 0, [int]$H = 0,
  [double]$Scale = 0.5
)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Dpi {
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
}
"@
[void][Dpi]::SetProcessDpiAwarenessContext([IntPtr](-4))
if ($W -le 0) { $W = [Dpi]::GetSystemMetrics(0) }
if ($H -le 0) { $H = [Dpi]::GetSystemMetrics(1) }
$bmp = New-Object System.Drawing.Bitmap $W, $H
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($X, $Y, 0, 0, (New-Object System.Drawing.Size $W, $H))
$g.Dispose()
if ($Scale -ne 1) {
  $sw = [int]($W * $Scale); $sh = [int]($H * $Scale)
  $small = New-Object System.Drawing.Bitmap $sw, $sh
  $g2 = [System.Drawing.Graphics]::FromImage($small)
  $g2.InterpolationMode = 'HighQualityBicubic'
  $g2.DrawImage($bmp, 0, 0, $sw, $sh)
  $g2.Dispose()
  $bmp.Dispose()
  $bmp = $small
}
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
"saved $Out"
