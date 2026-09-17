//! Stable, absolute storage paths for development and packaged launches.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub addr: String,
    /// The installation root: where runtime/bin and plugins live, whatever
    /// COMPANION_MODELS_DIR or COMPANION_DATA_DIR point at.
    pub root: PathBuf,
    pub data_dir: PathBuf,
    pub models_dir: PathBuf,
    pub frontend_dir: PathBuf,
}

impl AppConfig {
    pub fn from_env() -> Self {
        let cwd = std::env::current_dir().expect("cannot resolve current directory");
        let executable = std::env::current_exe().ok();
        let root = installation_root(&cwd, executable.as_deref());
        let addr = std::env::var("COMPANION_ADDR").unwrap_or_else(|_| "127.0.0.1:3877".into());
        let data_dir =
            env_dir("COMPANION_DATA_DIR", &cwd).unwrap_or_else(|| default_data_dir(&root));
        let models_dir =
            env_dir("COMPANION_MODELS_DIR", &cwd).unwrap_or_else(|| root.join("models"));
        let frontend_dir =
            env_dir("COMPANION_FRONTEND_DIR", &cwd).unwrap_or_else(|| root.join("frontend/dist"));
        let root_db = root.join("data/companion.db");
        let legacy_db = root.join("backend/data/companion.db");
        if root_db.is_file() && legacy_db.is_file() {
            tracing::warn!(
                active = %data_dir.display(),
                other_locations = %format!("{}, {}", root_db.display(), legacy_db.display()),
                "Two history databases exist. Neither has been moved or deleted. COMPANION_DATA_DIR selects the active history explicitly."
            );
        }
        Self {
            addr,
            root,
            data_dir,
            models_dir,
            frontend_dir,
        }
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("companion.db")
    }
}

fn env_dir(name: &str, cwd: &Path) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(|value| {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            }
        })
}

/// Executable location takes precedence over cwd: opening the app from a
/// different project must not create a different conversation database there.
fn installation_root(cwd: &Path, executable: Option<&Path>) -> PathBuf {
    for start in executable
        .and_then(Path::parent)
        .into_iter()
        .chain(std::iter::once(cwd))
    {
        for candidate in start.ancestors() {
            if candidate.join("backend/Cargo.toml").is_file() && candidate.join("frontend").is_dir()
            {
                return candidate.to_path_buf();
            }
        }
    }
    executable
        .and_then(Path::parent)
        .unwrap_or(cwd)
        .to_path_buf()
}

fn default_data_dir(root: &Path) -> PathBuf {
    // Preserve established cargo-run history. This choice is identical from
    // root/, backend/, or a shortcut; never merge or select by timestamps.
    let legacy = root.join("backend/data");
    if legacy.join("companion.db").is_file() {
        legacy
    } else {
        root.join("data")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_does_not_depend_on_launch_directory() {
        let root = fixture();
        let executable = root.join("backend/target/debug/companion-backend.exe");
        assert_eq!(installation_root(&root, Some(&executable)), root);
        assert_eq!(
            installation_root(&root.join("backend"), Some(&executable)),
            root
        );
        assert_eq!(
            installation_root(&std::env::temp_dir(), Some(&executable)),
            root
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!("companion-config-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("backend")).unwrap();
        std::fs::create_dir_all(root.join("frontend")).unwrap();
        std::fs::write(root.join("backend/Cargo.toml"), "[package]").unwrap();
        root
    }

    #[test]
    fn preserves_legacy_history_even_if_both_locations_exist() {
        let root = fixture();
        assert_eq!(default_data_dir(&root), root.join("data"));
        std::fs::create_dir_all(root.join("backend/data")).unwrap();
        std::fs::write(root.join("backend/data/companion.db"), "legacy").unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/companion.db"), "newer").unwrap();
        assert_eq!(default_data_dir(&root), root.join("backend/data"));
        assert_eq!(
            std::fs::read_to_string(root.join("data/companion.db")).unwrap(),
            "newer"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn packaged_root_uses_executable_location() {
        let temp = std::env::temp_dir();
        let root = temp.join("companion-installed-test");
        assert_eq!(
            installation_root(&temp, Some(&root.join("companion-backend.exe"))),
            root
        );
    }
}
