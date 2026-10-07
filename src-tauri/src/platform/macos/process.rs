//! 子进程启动与生命周期。

use std::path::Path;
use std::process::{Child, Command};

/// macOS 的子进程本来就不带窗口。
pub fn hidden_command(program: &Path) -> Command {
    Command::new(program)
}

/// macOS 版用系统自带的识字引擎，没有常驻的子进程，暂时不用做事。
/// 以后要带子进程的话：没有 Windows 那种作业对象，得在子进程里盯着父进程
/// （kqueue 的 `NOTE_EXIT`，或定时看 `getppid()` 有没有变成 1）。
pub fn tie_to_current_process(_child: &Child) {}
