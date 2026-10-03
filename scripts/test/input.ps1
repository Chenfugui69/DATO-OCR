# 测试用输入模拟：按键 / 鼠标移动 / 拖拽 / 点击。坐标是屏幕物理像素。
# 用法：
#   input.ps1 key F1            按一下 F1（可写 ctrl+z、shift+a、esc、enter）
#   input.ps1 move 100 200
#   input.ps1 drag 100 200 900 700
#   input.ps1 ctrldrag 100 200 900 700   按住 Ctrl 拖（altdrag 同理）
#   input.ps1 click 500 500 [right|double]
#   input.ps1 down 500 500 / input.ps1 up 600 600   分开按下、抬起左键（中途可以截图）
#   input.ps1 wheel -3          负数向下滚
#   input.ps1 type 你好
#
# 安全保护：点击 / 拖拽 / 滚轮之前检查鼠标下面那个窗口属于哪个进程，不在允许列表里就报错不执行，
# 免得测试时点到别人正在用的程序。默认只允许 DATO COR 自己和测试窗口（pwsh / powershell）；
# 要放宽就设环境变量 CHENOCR_TEST_ALLOW，例如 $env:CHENOCR_TEST_ALLOW = 'chenocr,pwsh,msedge'。
# 再设 CHENOCR_TEST_TITLE 的话，还要求窗口标题包含这段文字（区分测试用的浏览器窗口和你自己的）。
param([string]$Action, [string]$A, [string]$B, [string]$C, [string]$D)
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class In {
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, int data, UIntPtr extra);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern short VkKeyScan(char ch);
  [StructLayout(LayoutKind.Sequential)] public struct PT { public int X, Y; }
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(PT p);
  [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint flags);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out PT p);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, System.Text.StringBuilder s, int n);
}
"@
[void][In]::SetProcessDpiAwarenessContext([IntPtr](-4))
$vk = @{ f1=0x70; f2=0x71; f3=0x72; f4=0x73; esc=0x1B; escape=0x1B; enter=0x0D; tab=0x09; space=0x20; back=0x08; backspace=0x08; delete=0x2E;
         left=0x25; up=0x26; right=0x27; down=0x28; ctrl=0x11; shift=0x10; alt=0x12; win=0x5B }
$allow = if ($env:CHENOCR_TEST_ALLOW) { $env:CHENOCR_TEST_ALLOW.Split(',') | ForEach-Object { $_.Trim().ToLower() } } else { @('chenocr', 'dato cor', 'pwsh', 'powershell') }
function Guard([int]$x, [int]$y) {
  $p = New-Object In+PT; $p.X = $x; $p.Y = $y
  $h = [In]::GetAncestor([In]::WindowFromPoint($p), 2)
  $procId = 0; [void][In]::GetWindowThreadProcessId($h, [ref]$procId)
  $name = (Get-Process -Id $procId -ErrorAction SilentlyContinue).ProcessName
  if (-not $name -or $allow -notcontains $name.ToLower()) { throw "已拦截：($x,$y) 下面是 '$name' 的窗口，不在允许列表 [$($allow -join ',')] 里" }
  if ($env:CHENOCR_TEST_TITLE -and @('chenocr', 'dato cor', 'pwsh', 'powershell') -notcontains $name.ToLower()) {
    $sb = New-Object System.Text.StringBuilder 256; [void][In]::GetWindowText($h, $sb, 256)
    if (-not $sb.ToString().Contains($env:CHENOCR_TEST_TITLE)) { throw "已拦截：($x,$y) 下面的窗口标题是 '$sb'，不含 '$env:CHENOCR_TEST_TITLE'" }
  }
}
function Key([string]$combo) {
  $parts = $combo.ToLower().Split('+')
  $codes = foreach ($p in $parts) { if ($vk.ContainsKey($p)) { $vk[$p] } elseif ($p.Length -eq 1) { [byte][char]$p.ToUpper() } else { throw "unknown key $p" } }
  foreach ($c in $codes) { [In]::keybd_event([byte]$c, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 15 }
  [array]::Reverse($codes)
  foreach ($c in $codes) { [In]::keybd_event([byte]$c, 0, 2, [UIntPtr]::Zero); Start-Sleep -Milliseconds 15 }
}
switch ($Action) {
  'key' { Key $A }
  'move' { [void][In]::SetCursorPos([int]$A, [int]$B); [In]::mouse_event(0x0001, 1, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 20; [In]::mouse_event(0x0001, -1, 0, 0, [UIntPtr]::Zero) }
  'click' {
    Guard ([int]$A) ([int]$B)
    [void][In]::SetCursorPos([int]$A, [int]$B); Start-Sleep -Milliseconds 40
    if ($C -eq 'right') { [In]::mouse_event(0x08, 0, 0, 0, [UIntPtr]::Zero); [In]::mouse_event(0x10, 0, 0, 0, [UIntPtr]::Zero) }
    else {
      $n = if ($C -eq 'double') { 2 } else { 1 }
      for ($i = 0; $i -lt $n; $i++) { [In]::mouse_event(0x02, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 30; [In]::mouse_event(0x04, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 60 }
    }
  }
  { $_ -in 'drag', 'ctrldrag', 'altdrag' } {
    $x1 = [int]$A; $y1 = [int]$B; $x2 = [int]$C; $y2 = [int]$D
    $mod = @{ ctrldrag = 0x11; altdrag = 0x12 }[$Action]
    Guard $x1 $y1; Guard $x2 $y2
    if ($mod) { [In]::keybd_event([byte]$mod, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 30 }
    [void][In]::SetCursorPos($x1, $y1); Start-Sleep -Milliseconds 60
    [In]::mouse_event(0x02, 0, 0, 0, [UIntPtr]::Zero)
    for ($i = 1; $i -le 20; $i++) {
      [void][In]::SetCursorPos([int]($x1 + ($x2 - $x1) * $i / 20), [int]($y1 + ($y2 - $y1) * $i / 20)); Start-Sleep -Milliseconds 15
    }
    Start-Sleep -Milliseconds 40
    [In]::mouse_event(0x04, 0, 0, 0, [UIntPtr]::Zero)
    if ($mod) { Start-Sleep -Milliseconds 80; [In]::keybd_event([byte]$mod, 0, 2, [UIntPtr]::Zero) }
  }
  'down' { Guard ([int]$A) ([int]$B); [void][In]::SetCursorPos([int]$A, [int]$B); Start-Sleep -Milliseconds 40; [In]::mouse_event(0x02, 0, 0, 0, [UIntPtr]::Zero) }
  'up' {
    $x1 = [int]$A; $y1 = [int]$B
    $p = New-Object In+PT; [void][In]::GetCursorPos([ref]$p)
    for ($i = 1; $i -le 15; $i++) { [void][In]::SetCursorPos([int]($p.X + ($x1 - $p.X) * $i / 15), [int]($p.Y + ($y1 - $p.Y) * $i / 15)); Start-Sleep -Milliseconds 15 }
    [In]::mouse_event(0x04, 0, 0, 0, [UIntPtr]::Zero)
  }
  'wheel' { $p = New-Object In+PT; [void][In]::GetCursorPos([ref]$p); Guard $p.X $p.Y; [In]::mouse_event(0x0800, 0, 0, [int]$A * 120, [UIntPtr]::Zero) }
  'type' { Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.SendKeys]::SendWait($A) }
}
