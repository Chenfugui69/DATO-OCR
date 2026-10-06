# 列出某进程的所有有标题的顶层窗口：句柄、可见性、显示亲和性（17 = 不让截屏拍到）、位置。
param([string[]]$Process = @('chenocr', 'DATO OCR'))
Add-Type @"
using System; using System.Text; using System.Runtime.InteropServices;
public static class LW {
  public delegate bool P(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern bool EnumWindows(P p, IntPtr l);
  [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool GetWindowDisplayAffinity(IntPtr h, out uint a);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
"@
[void][LW]::SetProcessDpiAwarenessContext([IntPtr](-4))
$ids = @((Get-Process -Name $Process -ErrorAction SilentlyContinue).Id)
$list = New-Object System.Collections.ArrayList
$cb = [LW+P]{
  param($h, $l)
  $procId = 0; [void][LW]::GetWindowThreadProcessId($h, [ref]$procId)
  if ($ids -contains $procId) {
    $sb = New-Object Text.StringBuilder 256; [void][LW]::GetWindowText($h, $sb, 256)
    if ($sb.Length -gt 0) {
      $a = 0; [void][LW]::GetWindowDisplayAffinity($h, [ref]$a)
      $r = New-Object LW+RECT; [void][LW]::GetWindowRect($h, [ref]$r)
      [void]$list.Add(("{0} vis={1} min={2} aff={3} [{4}] {5},{6} {7}x{8}" -f $h, [LW]::IsWindowVisible($h), [LW]::IsIconic($h), $a, $sb, $r.L, $r.T, ($r.R - $r.L), ($r.B - $r.T)))
    }
  }
  $true
}
[void][LW]::EnumWindows($cb, [IntPtr]::Zero)
$list
