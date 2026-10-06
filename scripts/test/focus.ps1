# 把 DATO OCR 的某个窗口切到前台（AttachThreadInput 绕过前台锁）。
# 发热键前先用它：有的程序（远程控制、游戏）在前台时会吞掉全局热键。
# 同名窗口多时用 -Handle 指定句柄（winshot.ps1 -List 打出来的第一列）
param([string]$Title = "DATO OCR", [long]$Handle = 0)
Add-Type @"
using System; using System.Text; using System.Collections.Generic; using System.Runtime.InteropServices;
public static class Fz {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool f);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
  public static List<IntPtr> All() { var l = new List<IntPtr>(); EnumWindows((h, p) => { l.Add(h); return true; }, IntPtr.Zero); return l; }
}
"@
$procs = Get-Process -Name chenocr, 'DATO OCR' -ErrorAction SilentlyContinue | % { $_.Id }
foreach ($h in [Fz]::All()) {
  $p = 0; [void][Fz]::GetWindowThreadProcessId($h, [ref]$p)
  if ($procs -notcontains $p -or -not [Fz]::IsWindowVisible($h)) { continue }
  $sb = New-Object Text.StringBuilder 256; [void][Fz]::GetWindowText($h, $sb, 256)
  if ($Handle -ne 0) { if ($h.ToInt64() -ne $Handle) { continue } }
  elseif ($sb.ToString() -ne $Title) { continue }
  $fg = [Fz]::GetForegroundWindow(); $x = 0
  $fgThread = [Fz]::GetWindowThreadProcessId($fg, [ref]$x)
  $me = [Fz]::GetCurrentThreadId()
  [void][Fz]::AttachThreadInput($me, $fgThread, $true)
  [void][Fz]::BringWindowToTop($h); [void][Fz]::SetForegroundWindow($h)
  [void][Fz]::AttachThreadInput($me, $fgThread, $false)
  "focused $h"
  break
}
