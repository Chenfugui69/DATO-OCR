//! 数据目录布局（规格 07 §6）。
//!
//! ```text
//! %APPDATA%\DATO OCR\
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
//! 品牌改过两次名（2026-10）：CHENOCR → DATO COR → DATO OCR。旧目录在启动时整个改名过去。

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult};

/// 数据目录名（= 品牌名）
const DIR_NAME: &str = "DATO OCR";
/// 改名前的数据目录名，新的在前
const LEGACY_DIR_NAMES: &[&str] = &["DATO COR", "CHENOCR"];

#[derive(Clone, Debug)]
pub struct AppPaths {
    root: PathBuf,
    resources: Option<PathBuf>,
    /// 这次启动把旧目录搬了过来（启动日志里记一笔）
    pub migrated_from: Option<PathBuf>,
    /// 默认的数据目录（%APPDATA%\DATO OCR）。用户改过位置时这里只剩一个指向新位置的 `data-location.json`
    pub default_root: PathBuf,
    /// 这次启动按用户的要求搬了数据目录：Ok(从哪儿搬来) / Err(搬失败的原因，继续用原来的)
    pub relocated: Option<Result<PathBuf, String>>,
}

/// 默认数据目录里记着"数据实际放在哪"的文件（用户在设置 → 存储里改过位置才有）
const LOCATION_FILE: &str = "data-location.json";

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Location {
    /// 数据实际所在的目录
    root: Option<PathBuf>,
    /// 下次启动（打开数据库之前）要先从这里把数据搬到 `root`
    move_from: Option<PathBuf>,
}

fn read_location(default_root: &Path) -> Location {
    std::fs::read_to_string(default_root.join(LOCATION_FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_location(default_root: &Path, loc: &Location) -> std::io::Result<()> {
    std::fs::create_dir_all(default_root)?;
    let file = default_root.join(LOCATION_FILE);
    if loc.root.is_none() && loc.move_from.is_none() {
        return match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    std::fs::write(file, serde_json::to_vec_pretty(loc).unwrap_or_default())
}

/// 目录里有没有东西（默认目录里那个位置文件不算）
fn has_data(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|d| d.flatten().any(|e| e.file_name() != LOCATION_FILE))
}

/// 把 `from` 里的东西（temp、位置文件除外）复制到 `to`。上一个进程刚退出时文件可能还占着，重试几次
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == LOCATION_FILE || name == "temp" {
            continue;
        }
        let src = entry.path();
        let dst = to.join(&name);
        if entry.file_type()?.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            let mut tries = 0;
            loop {
                match std::fs::copy(&src, &dst) {
                    Ok(_) => break,
                    Err(_) if tries < 20 => {
                        tries += 1;
                        std::thread::sleep(std::time::Duration::from_millis(250));
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }
    Ok(())
}

/// 搬完删掉原来的：默认目录要留着（里面放位置文件），只清空；别的地方整个删掉
fn remove_old(from: &Path, default_root: &Path) {
    if from == default_root {
        if let Ok(entries) = std::fs::read_dir(from) {
            for e in entries.flatten() {
                if e.file_name() == LOCATION_FILE {
                    continue;
                }
                let p = e.path();
                let _ = if p.is_dir() {
                    std::fs::remove_dir_all(&p)
                } else {
                    std::fs::remove_file(&p)
                };
            }
        }
    } else {
        let _ = std::fs::remove_dir_all(from);
    }
}

/// 数据实际在哪：按位置文件来；有没搬完的就先搬（在打开数据库、写日志之前）
fn located_root(default_root: &Path) -> (PathBuf, Option<Result<PathBuf, String>>) {
    let mut loc = read_location(default_root);
    let target = loc.root.clone().unwrap_or_else(|| default_root.to_path_buf());
    let mut relocated = None;
    if let Some(from) = loc.move_from.take() {
        let result = if from == target {
            Ok(())
        } else if has_data(&target) {
            Err(format!("{} 里已经有东西了", target.display()))
        } else {
            copy_tree(&from, &target).map_err(|e| e.to_string())
        };
        match result {
            Ok(()) => {
                if from != target {
                    remove_old(&from, default_root);
                }
                // 搬回默认位置：位置文件也不要了
                if target == default_root {
                    loc.root = None;
                }
                relocated = Some(Ok(from));
            }
            Err(err) => {
                // 搬失败：复制了一半的新目录不要了，继续用原来的
                if from != target && target != default_root {
                    let _ = std::fs::remove_dir_all(&target);
                }
                loc.root = (from != default_root).then(|| from.clone());
                relocated = Some(Err(err));
            }
        }
        if let Err(err) = write_location(default_root, &loc) {
            eprintln!("写数据位置文件失败：{err}");
        }
    }
    let root = loc.root.unwrap_or_else(|| default_root.to_path_buf());
    // 指向的目录建不出来（移动硬盘没插）：这次先用默认位置，不改位置文件
    if root != default_root && std::fs::create_dir_all(&root).is_err() {
        eprintln!("数据目录 {} 用不了，这次先用默认位置", root.display());
        return (default_root.to_path_buf(), relocated);
    }
    (root, relocated)
}

/// 用户在设置里选了新的数据位置：记下来，重启后（打开数据库之前）搬过去。
/// `dir` 是用户选的文件夹，数据放在它下面的 `DATO OCR` 里；`None` = 搬回默认位置
pub fn request_move(paths: &AppPaths, dir: Option<&Path>) -> AppResult<PathBuf> {
    let target = match dir {
        None => paths.default_root.clone(),
        Some(d) if d.file_name().is_some_and(|n| n == DIR_NAME) => d.to_path_buf(),
        Some(d) => d.join(DIR_NAME),
    };
    let current = paths.root();
    if target == current {
        return Err(AppError::msg("数据已经在这个位置了"));
    }
    if target.starts_with(current) {
        return Err(AppError::msg("不能放到现在的数据目录里面"));
    }
    if current.starts_with(&target) {
        return Err(AppError::msg("不能放到现在数据目录的上一级"));
    }
    if has_data(&target) {
        return Err(AppError::msg(format!(
            "{} 里已经有东西了，换一个空文件夹",
            target.display()
        )));
    }
    // 先试试能不能写
    std::fs::create_dir_all(&target)
        .map_err(|e| AppError::msg(format!("建不了文件夹 {}：{e}", target.display())))?;
    let probe = target.join(".write-test");
    std::fs::write(&probe, b"ok")
        .map_err(|e| AppError::msg(format!("{} 写不进去：{e}", target.display())))?;
    let _ = std::fs::remove_file(probe);
    write_location(
        &paths.default_root,
        &Location {
            root: Some(target.clone()),
            move_from: Some(current.to_path_buf()),
        },
    )?;
    Ok(target)
}

/// 新目录还没有、旧目录在：整个改名过去。改名失败（旧版本还开着、文件被占用）就这次先用
/// 旧目录，下次启动再搬 —— 绝不能新建一个空目录，那样用户看起来像是数据全丢了。
fn data_root(base: &Path) -> (PathBuf, Option<PathBuf>) {
    let root = base.join(DIR_NAME);
    let legacy = LEGACY_DIR_NAMES
        .iter()
        .map(|n| base.join(n))
        .find(|p| p.exists());
    let Some(legacy) = legacy.filter(|_| !root.exists()) else {
        return (root, None);
    };
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
        // 测试开关：`CHENOCR_TEST_DATA_DIR=某个目录` 时数据放那里，不碰用户真正的历史和设置
        let base = match std::env::var_os("CHENOCR_TEST_DATA_DIR") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => app
                .path()
                .data_dir()
                .map_err(|err| AppError::msg(format!("找不到用户数据目录：{err}")))?,
        };
        let (default_root, migrated_from) = data_root(&base);
        let (root, relocated) = located_root(&default_root);
        let paths = Self {
            root,
            resources: app.path().resource_dir().ok().map(strip_verbatim),
            migrated_from,
            default_root,
            relocated,
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

    /// MCP 服务开着时写在这里的端口和令牌，`--mcp` 命令行桥接靠它找到服务
    pub fn mcp_file(&self) -> PathBuf {
        self.root.join("mcp.json")
    }

    /// 不经过 Tauri 推算数据目录（`--mcp` 桥接进程里用，它不启动界面）。和 `resolve` 的结果一致：
    /// 测试开关优先，否则 Windows 是 %APPDATA%，macOS 是 ~/Library/Application Support
    pub fn guess_root() -> Option<PathBuf> {
        let base = match std::env::var_os("CHENOCR_TEST_DATA_DIR") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => match std::env::var_os("APPDATA") {
                Some(appdata) => PathBuf::from(appdata),
                None => {
                    PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support")
                }
            },
        };
        let default_root = base.join(DIR_NAME);
        // 用户改过数据位置：跟着位置文件走（MCP 桥接要找的 mcp.json 在那边）
        Some(read_location(&default_root).root.unwrap_or(default_root))
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
            default_root: PathBuf::from("/data"),
            relocated: None,
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
        // 两个旧名字都在时搬最近的那个（DATO COR）
        std::fs::create_dir_all(base.join("CHENOCR")).unwrap();
        std::fs::create_dir_all(base.join("DATO COR").join("screenshots")).unwrap();
        std::fs::write(base.join("DATO COR").join("settings.json"), "{}").unwrap();

        let (root, moved) = data_root(&base);
        assert_eq!(root, base.join(DIR_NAME));
        assert_eq!(moved, Some(base.join("DATO COR")));
        assert!(root.join("settings.json").exists());
        assert!(root.join("screenshots").is_dir());
        assert!(!base.join("DATO COR").exists());

        // 第二次启动：新目录已在，什么都不做
        let (again, moved) = data_root(&base);
        assert_eq!(again, root);
        assert_eq!(moved, None);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn data_dir_moves_away_and_back() {
        let base = std::env::temp_dir().join(format!("datocor-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let default_root = base.join(DIR_NAME);
        std::fs::create_dir_all(default_root.join("screenshots/2026/11")).unwrap();
        std::fs::create_dir_all(default_root.join("temp")).unwrap();
        std::fs::write(default_root.join("chenocr.db"), "db").unwrap();
        std::fs::write(default_root.join("screenshots/2026/11/a.png"), "png").unwrap();
        let mut paths = AppPaths {
            root: default_root.clone(),
            resources: None,
            migrated_from: None,
            default_root: default_root.clone(),
            relocated: None,
        };

        // 选了 D:\Elsewhere → 数据放在 D:\Elsewhere\DATO OCR
        let target = request_move(&paths, Some(&base.join("Elsewhere"))).unwrap();
        assert_eq!(target, base.join("Elsewhere").join(DIR_NAME));
        // 搬到自己里面、原地不动都不行
        assert!(request_move(&paths, Some(&default_root.join("sub"))).is_err());

        // 重启：先搬再用
        let (root, relocated) = located_root(&default_root);
        assert_eq!(root, target);
        assert_eq!(relocated.unwrap().unwrap(), default_root);
        assert_eq!(std::fs::read_to_string(root.join("chenocr.db")).unwrap(), "db");
        assert!(root.join("screenshots/2026/11/a.png").exists());
        assert!(!root.join("temp").exists());
        // 默认目录只剩位置文件
        let left: Vec<_> = std::fs::read_dir(&default_root).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left, vec![std::ffi::OsString::from(LOCATION_FILE)]);
        // 再启动一次：不再搬
        let (again, relocated) = located_root(&default_root);
        assert_eq!(again, target);
        assert!(relocated.is_none());

        // 搬回默认位置：位置文件也删掉
        paths.root = target.clone();
        request_move(&paths, None).unwrap();
        let (back, relocated) = located_root(&default_root);
        assert_eq!(back, default_root);
        assert!(relocated.unwrap().is_ok());
        assert!(back.join("chenocr.db").exists());
        assert!(!back.join(LOCATION_FILE).exists());
        assert!(!target.exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
