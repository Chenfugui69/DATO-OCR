//! CoreGraphics / CoreFoundation / ApplicationServices 的 C 接口，只声明用到的。
//!
//! 这些都是稳定的 C ABI，自己声明比跟着绑定库的命名变动走省事；Objective-C 的类走 objc2。

#![allow(non_upper_case_globals, non_snake_case, dead_code)]

use std::ffi::c_void;

pub use objc2_core_foundation::{CGPoint, CGRect, CGSize};

pub type CGDirectDisplayID = u32;
pub type CGWindowID = u32;
pub type CFTypeRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFArrayRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFDataRef = *const c_void;
pub type CFMachPortRef = *mut c_void;
pub type CFRunLoopRef = *mut c_void;
pub type CFRunLoopSourceRef = *mut c_void;
pub type CGImageRef = *mut c_void;
pub type CGContextRef = *mut c_void;
pub type CGColorSpaceRef = *mut c_void;
pub type CGDataProviderRef = *mut c_void;
pub type CGDisplayModeRef = *mut c_void;
pub type CGEventRef = *mut c_void;
pub type CGEventSourceRef = *mut c_void;
pub type CGEventTapProxy = *mut c_void;

// CGWindowListOption
pub const kCGWindowListOptionOnScreenOnly: u32 = 1 << 0;
pub const kCGWindowListExcludeDesktopElements: u32 = 1 << 4;
pub const kCGNullWindowID: CGWindowID = 0;
// CGWindowImageOption
pub const kCGWindowImageBestResolution: u32 = 1 << 3;

// CGImageAlphaInfo
pub const kCGImageAlphaPremultipliedLast: u32 = 1;
pub const kCGImageAlphaLast: u32 = 3;
pub const kCGImageAlphaNoneSkipLast: u32 = 5;
// CGBlendMode / CGInterpolationQuality
pub const kCGBlendModeCopy: i32 = 17;
pub const kCGInterpolationNone: i32 = 1;
pub const kCGInterpolationHigh: i32 = 3;

// CGEventType
pub const kCGEventKeyDown: u32 = 10;
pub const kCGEventKeyUp: u32 = 11;
pub const kCGEventTapDisabledByTimeout: u32 = 0xFFFF_FFFE;
pub const kCGEventTapDisabledByUserInput: u32 = 0xFFFF_FFFF;
// CGEventField
pub const kCGKeyboardEventKeycode: u32 = 9;
// CGEventFlags
pub const kCGEventFlagMaskCommand: u64 = 0x0010_0000;
// CGEventTapLocation / Placement / Options
pub const kCGSessionEventTap: u32 = 1;
pub const kCGHeadInsertEventTap: u32 = 0;
pub const kCGEventTapOptionDefault: u32 = 0;
// CGEventSourceStateID
pub const kCGEventSourceStateCombinedSessionState: i32 = 0;
// CGEventFilterMask / CGEventSuppressionState
pub const kCGEventFilterMaskPermitLocalMouseEvents: u32 = 1;
pub const kCGEventFilterMaskPermitSystemDefinedEvents: u32 = 4;
pub const kCGEventSuppressionStateSuppressionInterval: u32 = 0;

// 虚拟键码（按 ANSI 键位）
pub const kVK_ANSI_C: u16 = 0x08;
pub const kVK_ANSI_V: u16 = 0x09;
pub const kVK_Return: i64 = 0x24;
pub const kVK_Delete: i64 = 0x33;
pub const kVK_Escape: i64 = 0x35;
pub const kVK_ANSI_KeypadEnter: i64 = 0x4C;

pub type CGEventTapCallBack = unsafe extern "C" fn(
    proxy: CGEventTapProxy,
    kind: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef;

pub type CGDataProviderReleaseDataCallback =
    Option<unsafe extern "C" fn(info: *mut c_void, data: *const c_void, size: usize)>;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    pub static kCGColorSpaceSRGB: CFStringRef;

    pub fn CGMainDisplayID() -> CGDirectDisplayID;
    pub fn CGGetActiveDisplayList(
        max: u32,
        displays: *mut CGDirectDisplayID,
        count: *mut u32,
    ) -> i32;
    pub fn CGDisplayBounds(display: CGDirectDisplayID) -> CGRect;
    pub fn CGDisplayCopyDisplayMode(display: CGDirectDisplayID) -> CGDisplayModeRef;
    pub fn CGDisplayModeGetPixelWidth(mode: CGDisplayModeRef) -> usize;
    pub fn CGDisplayModeGetWidth(mode: CGDisplayModeRef) -> usize;
    pub fn CGDisplayModeRelease(mode: CGDisplayModeRef);

    pub fn CGWindowListCopyWindowInfo(option: u32, relative_to: CGWindowID) -> CFArrayRef;
    pub fn CGWindowListCreateImage(
        bounds: CGRect,
        option: u32,
        window: CGWindowID,
        image_option: u32,
    ) -> CGImageRef;
    pub fn CGPreflightScreenCaptureAccess() -> bool;
    pub fn CGRequestScreenCaptureAccess() -> bool;

    pub fn CGImageGetWidth(image: CGImageRef) -> usize;
    pub fn CGImageGetHeight(image: CGImageRef) -> usize;
    pub fn CGImageRelease(image: CGImageRef);
    pub fn CGImageCreate(
        width: usize,
        height: usize,
        bits_per_component: usize,
        bits_per_pixel: usize,
        bytes_per_row: usize,
        space: CGColorSpaceRef,
        bitmap_info: u32,
        provider: CGDataProviderRef,
        decode: *const f64,
        should_interpolate: bool,
        intent: i32,
    ) -> CGImageRef;

    pub fn CGColorSpaceCreateWithName(name: CFStringRef) -> CGColorSpaceRef;
    pub fn CGColorSpaceRelease(space: CGColorSpaceRef);

    pub fn CGDataProviderCreateWithData(
        info: *mut c_void,
        data: *const c_void,
        size: usize,
        release: CGDataProviderReleaseDataCallback,
    ) -> CGDataProviderRef;
    pub fn CGDataProviderRelease(provider: CGDataProviderRef);

    pub fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: CGColorSpaceRef,
        bitmap_info: u32,
    ) -> CGContextRef;
    pub fn CGContextDrawImage(ctx: CGContextRef, rect: CGRect, image: CGImageRef);
    pub fn CGContextSetBlendMode(ctx: CGContextRef, mode: i32);
    pub fn CGContextSetInterpolationQuality(ctx: CGContextRef, quality: i32);
    pub fn CGContextRelease(ctx: CGContextRef);

    pub fn CGEventCreate(source: CGEventSourceRef) -> CGEventRef;
    pub fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
    pub fn CGEventSourceCreate(state: i32) -> CGEventSourceRef;
    pub fn CGEventSourceSetLocalEventsFilterDuringSuppressionState(
        source: CGEventSourceRef,
        filter: u32,
        state: u32,
    );
    pub fn CGEventCreateKeyboardEvent(
        source: CGEventSourceRef,
        key: u16,
        key_down: bool,
    ) -> CGEventRef;
    pub fn CGEventSetFlags(event: CGEventRef, flags: u64);
    pub fn CGEventPost(tap: u32, event: CGEventRef);
    pub fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    pub fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: CGEventTapCallBack,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    pub fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub static kCFRunLoopCommonModes: CFStringRef;
    pub static kCFRunLoopDefaultMode: CFStringRef;
    pub static kCFBooleanTrue: CFTypeRef;

    pub fn CFRelease(cf: CFTypeRef);
    pub fn CFGetTypeID(cf: CFTypeRef) -> usize;
    pub fn CFStringGetTypeID() -> usize;
    pub fn CFStringGetLength(string: CFStringRef) -> isize;
    pub fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: CFMachPortRef,
        order: isize,
    ) -> CFRunLoopSourceRef;
    pub fn CFMachPortInvalidate(port: CFMachPortRef);
    pub fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    pub fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    pub fn CFRunLoopRunInMode(mode: CFStringRef, seconds: f64, return_after_source: bool) -> i32;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    pub static kAXTrustedCheckOptionPrompt: CFStringRef;

    pub fn AXIsProcessTrusted() -> bool;
    pub fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    pub fn AXUIElementCreateSystemWide() -> CFTypeRef;
    pub fn AXUIElementSetMessagingTimeout(element: CFTypeRef, seconds: f32) -> i32;
    pub fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
}
