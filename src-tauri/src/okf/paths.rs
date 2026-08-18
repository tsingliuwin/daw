//! OKF 路径解析。基于 `app_root`（生产=~/.daw，测试=tempdir），可注入。

use std::path::{Path, PathBuf};

use crate::okf::model::{Category, Scope};

/// OKF 路径根。所有路径相对它解析；测试可注入 tempdir。
#[derive(Debug, Clone)]
pub struct OkfPaths {
    pub app_root: PathBuf,
}

impl OkfPaths {
    pub fn new(app_root: PathBuf) -> Self {
        Self { app_root }
    }

    /// 生产环境：根 = `~/.daw`（来自 `db::get_app_dir`）。
    pub fn production() -> Self {
        Self::new(crate::db::get_app_dir().unwrap_or_default())
    }

    /// 全局 OKF 目录：`<root>/okf`。
    pub fn global_okf_dir(&self) -> PathBuf {
        self.app_root.join("okf")
    }

    /// 工作区 OKF 目录：`<resolved_ws_dir>/okf`。
    /// `ws` 可以是相对键（如 "DefaultProject"，拼到 root 下）或绝对路径（自定义工作区）。
    pub fn workspace_okf_dir(&self, ws: &str) -> PathBuf {
        resolve_dir_under(&self.app_root, ws).join("okf")
    }

    /// 某类别在某作用域下的目录。
    pub fn category_dir(&self, scope: Scope, ws: &str, category: Category) -> PathBuf {
        let base = match scope {
            Scope::Global => self.global_okf_dir(),
            Scope::Workspace => self.workspace_okf_dir(ws),
        };
        base.join(category.dir())
    }
}

/// 把 `ws`（相对键或绝对路径）解析为工作区目录。
/// 绝对路径原样返回；相对键拼到 `root` 下。
pub fn resolve_dir_under(root: &Path, ws: &str) -> PathBuf {
    let p = Path::new(ws);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(ws)
    }
}

/// 供 commands.rs 使用的入口：把 `workspaces.path` 值解析为真实目录。
/// 相对键 → `~/.daw/<key>`；绝对路径原样返回。
pub fn resolve_workspace_dir(path: &str) -> Result<PathBuf, String> {
    let p = Path::new(path);
    if p.is_absolute() {
        return Ok(p.to_path_buf());
    }
    let root = crate::db::get_app_dir()?;
    Ok(root.join(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_key_joined_under_root() {
        let root = Path::new("ROOT");
        assert_eq!(resolve_dir_under(root, "DefaultProject"), Path::new("ROOT").join("DefaultProject"));
    }

    #[test]
    fn absolute_path_passthrough() {
        let abs = if cfg!(windows) { "C:/custom/ws" } else { "/custom/ws" };
        assert!(resolve_dir_under(Path::new("ROOT"), abs).is_absolute());
    }

    #[test]
    fn workspace_okf_dir_appends_okf() {
        let paths = OkfPaths::new(PathBuf::from("ROOT"));
        let dir = paths.workspace_okf_dir("DefaultProject");
        assert_eq!(dir, Path::new("ROOT").join("DefaultProject").join("okf"));
    }

    #[test]
    fn category_dir_global_vs_workspace() {
        let paths = OkfPaths::new(PathBuf::from("ROOT"));
        assert_eq!(
            paths.category_dir(Scope::Global, "ws", Category::Concept),
            Path::new("ROOT").join("okf").join("concepts")
        );
        assert_eq!(
            paths.category_dir(Scope::Workspace, "ws", Category::Table),
            Path::new("ROOT").join("ws").join("okf").join("tables")
        );
    }
}
