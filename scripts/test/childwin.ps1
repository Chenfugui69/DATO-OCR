# 列出某进程顶层窗口下的所有子窗口：类名、可见性、物理坐标矩形。排查"自动框选为什么框不到某个区域"用。
# 用法：childwin.ps1 -Process msedge
param([string]$Process = 'msedge')
Add-Type @"
using System; using System.Text; using System.Collections.Generic; using System.Runtime.InteropServices;
public static class CW {
  public delegate bool P(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern bool EnumWindows(P p, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, P p, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  public static List<IntPtr> Top() { var l = new List<IntPtr>(); EnumWindows((h, p) => { l.Add(h); return true; }, IntPtr.Zero); return l; }
  public static List<IntPtr> Kids(IntPtr w) { var l = new List<IntPtr>(); EnumChildWindows(w, (h, p) => { l.Add(h); return true; }, IntPtr.Zero); return l; }
  public static string Cls(IntPtr h) { var sb = new StringBuilder(256); GetClassName(h, sb, 256); return sb.ToString(); }
  public static string Txt(IntPtr h) { var sb = new StringBuilder(256); GetWindowText(h, sb, 256); return sb.ToString(); }
}
"@
[void][CW]::SetProcessDpiAwarenessContext([IntPtr](-4))
$ids = @((Get-Process $Process -ErrorAction Stop).Id)
foreach ($h in [CW]::Top()) {
  $procId = 0; [void][CW]::GetWindowThreadProcessId($h, [ref]$procId)
  if ($ids -notcontains $procId -or -not [CW]::IsWindowVisible($h)) { continue }
  $r = New-Object CW+RECT; [void][CW]::GetWindowRect($h, [ref]$r)
  "TOP {0} [{1}] '{2}' {3},{4} {5}x{6}" -f $h, [CW]::Cls($h), [CW]::Txt($h), $r.L, $r.T, ($r.R - $r.L), ($r.B - $r.T)
  foreach ($c in [CW]::Kids($h)) {
    $r = New-Object CW+RECT; [void][CW]::GetWindowRect($c, [ref]$r)
    "   child {0} parent={1} [{2}] vis={3} {4},{5} {6}x{7}" -f $c, [CW]::GetParent($c), [CW]::Cls($c), [CW]::IsWindowVisible($c), $r.L, $r.T, ($r.R - $r.L), ($r.B - $r.T)
  }
}
