//! 数据目录布局（规格 07 §6）。
//!
//! ```text
//! %APPDATA%\DATO COR\
//! ├── chenocr.db          主数据库（文件名是内部代号，没跟着品牌改）
//! ├── settings.json       配置（不含密钥）
//! ├── clipboard/YYYY/MM/  剪贴板图片与缩略图
//! ├── screenshots/YYYY/MM/ 截图库
//! ├── ocr/YYYY/MM/        识字原图
//! ├── app-icons/          来源应用图标缓存
//! ├── logs/               日志，按天滚动
//! └── temp/               启动时清空
//! ```
//!
//! 数据库里只存**相对路径**（相对 root），换数据目录时不用改库。
//!
//! 品牌从 CHENOCR 改名为 DATO COR（2026-10）：旧目录 `%APPDATA%\CHENOCR` 在启动时整个改名过去。

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult};

/// 数据目录名（= 品牌名）
const DIR_NAME: &str = "DATO COR";
/// 改名前的数据目录名
const LEGACY_DIR_NAME: &str = "CHENOCR";

#[derive(Clone, Debug)]
pub struct AppPaths {
    root: PathBuf,
    resources: Option<PathBuf>,
    /// 这次启动把旧目录搬了过来（启动日志里记一笔）
    pub migrated_from: Option<PathBuf>,
}

/// 新目录还没有、旧目录在：整个改名过去。改名失败（旧版本还开着、文件被占用）就这次先用
/// 旧目录，下次启动再搬 —— 绝不能新建一个空目录，那样用户看起来像是数据全丢了。
fn data_root(base: &Path) -> (PathBuf, Option<PathBuf>) {
    let root = base.join(DIR_NAME);
    let legacy = base.join(LEGACY_DIR_NAME);
    if root.exists() || !legacy.exists() {
        return (root, None);
    }
    match std::fs::rename(&legacy, &root) {
        Ok(()) => (root, Some(legacy)),
        Err(err) => {
            eprintln!("搬迁旧数据目录失败，这次继续用旧目录：{err}");
            (legacy, None)
        }
    }
}

impl AppPaths {
    pub fn resolve(app: &AppHandle) -> AppResult<Self> {
        let base = app
            .path()
            .data_dir()
            .map_err(|err| AppError::msg(format!("找不到用户数据目录：{err}")))?;
        let (root, migrated_from) = data_root(&base);
        let paths = Self {
            root,
            resources: app.path().resource_dir().ok().map(strip_verbatim),
            migrated_from,
        };
        for dir in [
            paths.root.clone(),
            paths.logs(),
            paths.temp(),
            paths.root.join("clipboard"),
            paths.root.join("screenshots"),
            paths.root.join("ocr"),
            paths.app_icons(),
        ] {
            std::fs::create_dir_all(&dir)?;
        }
        Ok(paths)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn db_file(&self) -> PathBuf {
        self.root.join("chenocr.db")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn temp(&self) -> PathBuf {
        self.root.join("temp")
    }

    pub fn app_icons(&self) -> PathBuf {
        self.root.join("app-icons")
    }

    /// 打包进安装目录的资源（OCR 引擎等）。
    pub fn resource(&self, rel: &str) -> Option<PathBuf> {
        let dir = self.resources.as_ref()?;
        let path = dir.join(rel);
        path.exists().then_some(path)
    }

    /// 生成一个新的分月存储相对路径，如 `screenshots/2026/10/<uuid>.png`。
    pub fn new_rel_file(&self, category: &str, ext: &str) -> AppResult<String> {
        let now = chrono::Local::now();
        let dir = format!("{category}/{}", now.format("%Y/%m"));
        std::fs::create_dir_all(self.root.join(&dir))?;
        Ok(format!("{dir}/{}.{ext}", uuid::Uuid::new_v4().simple()))
    }

    /// 相对路径 → 绝对路径。拒绝跳出数据目录的路径。
    pub fn abs(&self, rel: &str) -> PathBuf {
        let clean: PathBuf = Path::new(rel)
            .components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .collect();
        self.root.join(clean)
    }

    pub fn reset_temp(&self) -> AppResult<()> {
        let temp = self.temp();
        if temp.exists() {
            std::fs::remove_dir_all(&temp)?;
        }
        std::fs::create_dir_all(&temp)?;
        Ok(())
    }

    pub fn temp_file(&self, ext: &str) -> PathBuf {
        self.temp()
            .join(format!("{}.{ext}", uuid::Uuid::new_v4().simple()))
    }
}

/// 去掉 Windows 的 `\\?\` 长路径前缀。Tauri 的 resource_dir 会带上它，而很多外部程序
/// （比如 RapidOCR 用的 C 运行时文件 API）不认这种写法，会报"文件不存在"。
fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        p
    }
}

/// 缩略图路径：`a/b/c.png` → `a/b/c.thumb.jpg`
pub fn thumb_rel(rel: &str) -> String {
    match rel.rfind('.') {
        Some(dot) => format!("{}.thumb.jpg", &rel[..dot]),
        None => format!("{rel}.thumb.jpg"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_path_replaces_extension() {
        assert_eq!(thumb_rel("a/2026/10/x.png"), "a/2026/10/x.thumb.jpg");
        assert_eq!(thumb_rel("noext"), "noext.thumb.jpg");
    }

    #[test]
    fn verbatim_prefix_is_stripped() {
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\D:\a\b")),
            PathBuf::from(r"D:\a\b")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\UNC\srv\share")),
            PathBuf::from(r"\\srv\share")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"D:\x")),
            PathBuf::from(r"D:\x")
        );
    }

    #[test]
    fn abs_rejects_parent_components() {
        let paths = AppPaths {
            root: PathBuf::from("/data"),
            resources: None,
            migrated_from: None,
        };
        assert_eq!(
            paths.abs("../../etc/passwd"),
            PathBuf::from("/data/etc/passwd")
        );
        assert_eq!(paths.abs("a/b.png"), PathBuf::from("/data/a/b.png"));
    }

    #[test]
    fn legacy_data_dir_is_moved_once() {
        let base = std::env::temp_dir().join(format!("datocor-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join(LEGACY_DIR_NAME).join("screenshots")).unwrap();
        std::fs::write(base.join(LEGACY_DIR_NAME).join("settings.json"), "{}").unwrap();

        let (root, moved) = data_root(&base);
        assert_eq!(root, base.join(DIR_NAME));
        assert_eq!(moved, Some(base.join(LEGACY_DIR_NAME)));
        assert!(root.join("settings.json").exists());
        assert!(root.join("screenshots").is_dir());
        assert!(!base.join(LEGACY_DIR_NAME).exists());

        // 第二次启动：新目录已在，什么都不做
        let (again, moved) = data_root(&base);
        assert_eq!(again, root);
        assert_eq!(moved, None);
        let _ = std::fs::remove_dir_all(&base);
    }
}
