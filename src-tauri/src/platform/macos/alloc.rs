//! 大块内存直接向系统要、用完直接还。
//!
//! 一块屏的画面就是几十 MB（5K 屏 59MB），截一次图要分配好几块。macOS 的分配器放掉这种大块后
//! 并不还给系统，一直算在进程头上（实测截一次图主进程多占八九十兆，`malloc_zone_pressure_relief`
//! 也要不回来）。所以超过 `DIRECT` 的分配绕过它，直接 mmap / munmap，放掉就真的放掉。

use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::c_void;

/// 4MB 以上的走系统调用：这种大小一次截图才几块，多一次系统调用无所谓。
const DIRECT: usize = 4 << 20;
const PAGE: usize = 16 << 10;

const PROT_READ: i32 = 0x1;
const PROT_WRITE: i32 = 0x2;
const MAP_PRIVATE: i32 = 0x2;
const MAP_ANON: i32 = 0x1000;

extern "C" {
    fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> i32;
}

struct Allocator;

#[global_allocator]
static GLOBAL: Allocator = Allocator;

fn direct(layout: &Layout) -> bool {
    layout.size() >= DIRECT && layout.align() <= PAGE
}

unsafe fn map(size: usize) -> *mut u8 {
    // SAFETY: 匿名私有映射，不涉及文件；失败返回 MAP_FAILED（-1）。
    let p = unsafe {
        mmap(
            std::ptr::null_mut(),
            size,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANON,
            -1,
            0,
        )
    };
    if p as isize == -1 {
        std::ptr::null_mut()
    } else {
        p.cast()
    }
}

// SAFETY: 小块原样交给系统分配器；大块用 mmap（页对齐，满足 `direct` 限定的对齐），
// 释放时按同一个 layout 判断走哪条路，两边不会混。
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if direct(&layout) {
            unsafe { map(layout.size()) }
        } else {
            unsafe { System.alloc(layout) }
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if direct(&layout) {
            // 新映射的页本来就是零
            unsafe { map(layout.size()) }
        } else {
            unsafe { System.alloc_zeroed(layout) }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if direct(&layout) {
            unsafe { munmap(ptr.cast(), layout.size()) };
        } else {
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        if !direct(&layout) && !direct(&new_layout) {
            return unsafe { System.realloc(ptr, layout, new_size) };
        }
        let new = unsafe { self.alloc(new_layout) };
        if !new.is_null() {
            unsafe {
                std::ptr::copy_nonoverlapping(ptr, new, layout.size().min(new_size));
                self.dealloc(ptr, layout);
            }
        }
        new
    }
}
