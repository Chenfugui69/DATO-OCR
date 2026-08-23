//! 平台中立的共享数据类型。
//!
//! 契约见 `docs/spec/07-跨平台抽象层与数据存储.md` 第 3 节。
//! 这里**不允许**出现任何 Windows / macOS 专有类型，句柄一律用 `u64` 包装。

use serde::{Deserialize, Serialize};

/// 不透明的显示器标识。Windows 上是 `HMONITOR`，macOS 上是 `CGDirectDisplayID`。
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MonitorId(pub u64);

impl std::fmt::Display for MonitorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 物理像素坐标，原点是虚拟桌面左上角。
///
/// `x` / `y` 可能为负 —— 副屏排列在主屏左侧或上方时就会这样。所有跨屏几何计算
/// 都必须在这个坐标系里做，不要在逻辑像素里算。
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl PhysicalRect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn right(&self) -> i32 {
        self.x + self.width as i32
    }

    pub const fn bottom(&self) -> i32 {
        self.y + self.height as i32
    }

    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// 与另一个矩形的交集，无交集时返回 `None`。
    pub fn intersection(&self, other: &PhysicalRect) -> Option<PhysicalRect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());

        if right <= x || bottom <= y {
            return None;
        }

        Some(PhysicalRect::new(
            x,
            y,
            (right - x) as u32,
            (bottom - y) as u32,
        ))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorInfo {
    pub id: MonitorId,
    pub name: String,
    pub bounds: PhysicalRect,
    /// 排除任务栏后的可用区域。
    pub work_area: PhysicalRect,
    pub scale_factor: f64,
    pub is_primary: bool,
    pub refresh_rate: Option<u32>,
}

// `BackdropKind` / `VisualCapabilities`（规格 07 §3）等 M6 做玻璃材质探测和
// `get_visual_capabilities` command 时按原文补上。现在加进来只会是没人用的死代码。
