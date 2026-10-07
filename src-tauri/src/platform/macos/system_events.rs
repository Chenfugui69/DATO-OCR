//! 系统事件监听：显示器变化、睡眠唤醒、深浅色切换。
//!
//! 三个通知分别来自三个通知中心（应用自己的、工作区的、跨进程的），回调在发通知的线程上
//! 执行，上层的处理函数必须立刻返回。

use std::ptr::NonNull;
use std::sync::OnceLock;

use block2::RcBlock;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{
    NSDistributedNotificationCenter, NSNotification, NSNotificationCenter, NSString,
};

use super::geometry;
use super::util::on_main_async;
use crate::platform::SystemEvent;

static HANDLER: OnceLock<fn(SystemEvent)> = OnceLock::new();

fn fire(event: SystemEvent) {
    if let Some(handler) = HANDLER.get() {
        handler(event);
    }
}

pub fn install(handler: fn(SystemEvent)) {
    if HANDLER.set(handler).is_err() {
        return;
    }
    on_main_async(|_| {
        let observe = |center: &NSNotificationCenter, name: &str, event: SystemEvent| {
            let block = RcBlock::new(move |_: NonNull<NSNotification>| {
                if event == SystemEvent::DisplayChanged {
                    on_main_async(geometry::refresh_extras);
                }
                fire(event);
            });
            // SAFETY: 不指定对象和队列，回调在发通知的线程上执行。
            let token = unsafe {
                center.addObserverForName_object_queue_usingBlock(
                    Some(&NSString::from_str(name)),
                    None,
                    None,
                    &block,
                )
            };
            // 监听一直到进程退出，句柄不回收
            std::mem::forget(token);
        };
        observe(
            &NSNotificationCenter::defaultCenter(),
            "NSApplicationDidChangeScreenParametersNotification",
            SystemEvent::DisplayChanged,
        );
        observe(
            &NSWorkspace::sharedWorkspace().notificationCenter(),
            "NSWorkspaceDidWakeNotification",
            SystemEvent::Resumed,
        );
        observe(
            &NSDistributedNotificationCenter::defaultCenter(),
            "AppleInterfaceThemeChangedNotification",
            SystemEvent::ThemeChanged,
        );
    });
}
