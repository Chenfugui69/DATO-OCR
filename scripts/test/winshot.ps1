# 按标题截 DATO OCR 的某个窗口（PrintWindow），或 -List 列出它的所有顶层窗口。
# 用法：winshot.ps1 -Title "文字识别 - DATO OCR" -Out a.png   |   winshot.ps1 -List
param(
  [string]$Title = "DATO OCR",
  [string]$Out = "$PSScriptRoot\win.png",
  [double]$Scale = 0.6,
  [switch]$List
)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class W {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  public static List<IntPtr> All() { var l = new List<IntPtr>(); EnumWindows((h, p) => { l.Add(h); return true; }, IntPtr.Zero); return l; }
  public static string Text(IntPtr h) { var sb = new StringBuilder(256); GetWindowText(h, sb, 256); return sb.ToString(); }
}
"@
[void][W]::SetProcessDpiAwarenessContext([IntPtr](-4))
$procs = Get-Process -Name chenocr, 'DATO OCR' -ErrorAction SilentlyContinue | ForEach-Object { $_.Id }
$wins = [W]::All() | Where-Object {
  $pid0 = 0; [void][W]::GetWindowThreadProcessId($_, [ref]$pid0); $procs -contains $pid0
}
if ($List) {
  foreach ($h in $wins) {
    $r = New-Object W+RECT; [void][W]::GetWindowRect($h, [ref]$r)
    "{0} vis={1} [{2}] {3},{4} {5}x{6}" -f $h, [W]::IsWindowVisible($h), [W]::Text($h), $r.L, $r.T, ($r.R-$r.L), ($r.B-$r.T)
  }
  return
}
$target = $wins | Where-Object { [W]::IsWindowVisible($_) -and ([W]::Text($_) -like $Title) } | Select-Object -First 1
if (-not $target) { "no window matching $Title"; return }
$r = New-Object W+RECT; [void][W]::GetWindowRect($target, [ref]$r)
$w = $r.R - $r.L; $h = $r.B - $r.T
$bmp = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
[void][W]::PrintWindow($target, $hdc, 2)
$g.ReleaseHdc($hdc); $g.Dispose()
if ($Scale -ne 1) {
  $sw = [int]($w * $Scale); $sh = [int]($h * $Scale)
  $small = New-Object System.Drawing.Bitmap $sw, $sh
  $g2 = [System.Drawing.Graphics]::FromImage($small)
  $g2.InterpolationMode = 'HighQualityBicubic'
  $g2.DrawImage($bmp, 0, 0, $sw, $sh); $g2.Dispose(); $bmp.Dispose(); $bmp = $small
}
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
"saved $Out ($w x $h)"
