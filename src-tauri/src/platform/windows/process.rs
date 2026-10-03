//! 子进程启动与生命周期。

use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn hidden_command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// 进程级的作业对象，句柄故意不关：本进程退出时系统关掉它，作业里的子进程随之被杀。
fn job() -> Option<HANDLE> {
    static JOB: OnceLock<Option<isize>> = OnceLock::new();
    let raw = JOB.get_or_init(|| {
        // SAFETY: 创建匿名作业对象并设置限制；结构体大小如实传入。
        unsafe {
            let job = CreateJobObjectW(None, PCWSTR::null()).ok()?;
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .ok()?;
            Some(job.0 as isize)
        }
    });
    raw.map(|h| HANDLE(h as *mut std::ffi::c_void))
}

/// 让子进程跟本进程同生共死。正常退出时上层会自己关掉子进程，这里兜的是崩溃
/// （release 是 panic=abort，不走退出流程）和被任务管理器结束这两种情况。
pub fn tie_to_current_process(child: &Child) {
    let Some(job) = job() else {
        tracing::warn!("创建作业对象失败，子进程可能在崩溃后残留");
        return;
    };
    let process = HANDLE(child.as_raw_handle());
    // SAFETY: 两个句柄都有效；子进程句柄由 Child 持有，调用期间不会被关闭。
    if let Err(err) = unsafe { AssignProcessToJobObject(job, process) } {
        tracing::warn!("子进程加入作业对象失败：{err}");
    }
}
