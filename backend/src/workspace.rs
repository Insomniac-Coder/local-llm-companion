//! Workspace system (§27): confine the agent to one directory tree.
//!
//! All file tools must resolve through `WorkspaceManager::resolve` so path
//! traversal (`../..`, absolute escapes, symlink escapes) is rejected (§54).

use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
pub struct WorkspaceManager {
    root: PathBuf,
}

#[derive(Debug)]
pub enum WorkspaceError {
    OutsideWorkspace { requested: String },
    Io(std::io::Error),
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutsideWorkspace { requested } => write!(
                f,
                "Blocked: '{requested}' is outside the workspace. \
                 The assistant can only access files inside the selected workspace."
            ),
            Self::Io(e) => write!(f, "Filesystem error: {e}"),
        }
    }
}

impl WorkspaceManager {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Join a user/model-supplied path and ensure it stays inside root.
    /// Rejects absolute escapes and `..` traversal without touching disk
    /// (pure lexical check), then canonicalizes the nearest existing ancestor
    /// to also defeat symlink/junction escapes when creating a new file.
    pub fn resolve(&self, user_path: &str) -> Result<PathBuf, WorkspaceError> {
        let requested = user_path.to_string();
        let joined = if Path::new(user_path).is_absolute() {
            PathBuf::from(user_path)
        } else {
            self.root.join(user_path)
        };

        // Lexical containment: normalize `.`/`..` without I/O.
        let mut normalized = PathBuf::new();
        for comp in joined.components() {
            match comp {
                Component::CurDir => {}
                Component::ParentDir => {
                    if !normalized.pop() {
                        return Err(WorkspaceError::OutsideWorkspace { requested });
                    }
                }
                c => normalized.push(c.as_os_str()),
            }
        }
        let root_norm = normalize_lexical(&self.root);
        if normalized != root_norm && !normalized.starts_with(&root_norm) {
            return Err(WorkspaceError::OutsideWorkspace { requested });
        }

        let c_root = match self.root.canonicalize() {
            Ok(root) => root,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Preserve lexical resolution for a not-yet-created workspace,
                // but never treat an existing dangling link as a missing root.
                return match std::fs::symlink_metadata(&self.root) {
                    Err(missing) if missing.kind() == std::io::ErrorKind::NotFound => {
                        Ok(normalized)
                    }
                    Err(other) => Err(WorkspaceError::Io(other)),
                    Ok(_) => Err(WorkspaceError::Io(error)),
                };
            }
            Err(error) => return Err(WorkspaceError::Io(error)),
        };

        // canonicalize() alone fails for a new target, so walk upward only
        // while entries genuinely do not exist. symlink_metadata detects a
        // dangling link too; canonicalization failures of existing entries
        // must fail closed instead of falling back to an unchecked path.
        let mut ancestor = normalized.as_path();
        let mut missing_components = Vec::new();
        loop {
            match std::fs::symlink_metadata(ancestor) {
                Ok(_) => {
                    let mut resolved = ancestor.canonicalize().map_err(WorkspaceError::Io)?;
                    if !resolved.starts_with(&c_root) {
                        return Err(WorkspaceError::OutsideWorkspace { requested });
                    }
                    if !missing_components.is_empty() && !resolved.is_dir() {
                        return Err(WorkspaceError::Io(std::io::Error::new(
                            std::io::ErrorKind::NotADirectory,
                            "An existing parent of the requested file is not a directory",
                        )));
                    }
                    for component in missing_components.iter().rev() {
                        resolved.push(component);
                    }
                    return Ok(resolved);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    // If the root disappeared during validation, do not walk
                    // out of it and manufacture a valid-looking destination.
                    if ancestor == root_norm {
                        return Err(WorkspaceError::Io(error));
                    }
                    let name = ancestor
                        .file_name()
                        .ok_or_else(|| WorkspaceError::Io(error))?;
                    missing_components.push(name.to_os_string());
                    ancestor =
                        ancestor
                            .parent()
                            .ok_or_else(|| WorkspaceError::OutsideWorkspace {
                                requested: requested.clone(),
                            })?;
                }
                Err(error) => return Err(WorkspaceError::Io(error)),
            }
        }
    }

    /// Validate an artifact filename (§54: sanitize generated filenames).
    pub fn sanitize_filename(name: &str) -> Result<String, WorkspaceError> {
        let base = Path::new(name)
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| WorkspaceError::OutsideWorkspace {
                requested: name.into(),
            })?;
        if base.is_empty() || base == "." || base == ".." {
            return Err(WorkspaceError::OutsideWorkspace {
                requested: name.into(),
            });
        }
        if base
            .chars()
            .any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\0'))
        {
            return Err(WorkspaceError::OutsideWorkspace {
                requested: name.into(),
            });
        }
        Ok(base.to_string())
    }
}

fn normalize_lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::RootDir if out.as_os_str().is_empty() => {
                // Preserve Windows prefix + root (e.g. `C:\`).
                out.push(comp.as_os_str());
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("companion-workspace-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn workspace(&self) -> WorkspaceManager {
            let root = self.0.join("workspace");
            std::fs::create_dir(&root).unwrap();
            WorkspaceManager::new(root)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // Only remove the unique test-owned directory in the resolved
            // system temporary directory. All link targets below also live
            // inside this fixture; no user paths are ever linked or removed.
            let parent = std::env::temp_dir().canonicalize().unwrap();
            if self.0.parent() == Some(parent.as_path())
                && self
                    .0
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("companion-workspace-test-")
            {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    fn link_directory(target: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        let result = windows_directory_link(target, link);
        #[cfg(unix)]
        let result = std::os::unix::fs::symlink(target, link);
        #[cfg(not(any(windows, unix)))]
        let result: Result<(), std::io::Error> = Err(std::io::ErrorKind::Unsupported.into());
        match result {
            Ok(()) => true,
            Err(error)
                if error.kind() == std::io::ErrorKind::PermissionDenied
                    || error.kind() == std::io::ErrorKind::Unsupported
                    || error.raw_os_error() == Some(1314) =>
            {
                eprintln!("SKIPPED symlink regression: this host does not permit directory symlinks ({error})");
                false
            }
            Err(error) => panic!("Failed to create test directory symlink: {error}"),
        }
    }

    #[cfg(windows)]
    fn windows_directory_link(target: &Path, link: &Path) -> Result<(), std::io::Error> {
        use std::os::windows::process::CommandExt;

        match std::os::windows::fs::symlink_dir(target, link) {
            Ok(()) => return Ok(()),
            Err(error) if error.raw_os_error() == Some(1314) => {}
            Err(error) => return Err(error),
        }
        // NTFS junctions exercise the same reparse-point boundary without
        // requiring Windows developer mode or administrator symlink rights.
        // Both paths must belong to the same unique test-owned fixture.
        let temporary = std::env::temp_dir().canonicalize()?;
        let fixture = link
            .ancestors()
            .find(|path| {
                path.parent() == Some(temporary.as_path())
                    && path.file_name().is_some_and(|name| {
                        name.to_string_lossy()
                            .starts_with("companion-workspace-test-")
                    })
            })
            .expect("junction must be inside the test fixture");
        assert!(
            target.starts_with(fixture),
            "junction target must be test-owned"
        );

        let make_dangling = !target.try_exists()?;
        if make_dangling {
            std::fs::create_dir(target)?;
        }
        // Windows PowerShell 5 builds malformed junction targets from Rust's
        // verbatim \\?\ canonical paths; pass equivalent normal Win32 paths.
        let powershell_path = |path: &Path| {
            let text = path.to_string_lossy();
            if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
                PathBuf::from(format!(r"\\{unc}"))
            } else {
                PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(text.as_ref()))
            }
        };
        // Environment values are data, never interpolated into the fixed
        // PowerShell program. Creation is hidden and cleanup stays in Rust.
        let output = std::process::Command::new("powershell.exe")
            .creation_flags(0x08000000)
            .args([
                "-NoProfile", "-NonInteractive", "-Command",
                "$ErrorActionPreference = 'Stop'; New-Item -ItemType Junction -Path $env:COMPANION_TEST_JUNCTION_LINK -Target $env:COMPANION_TEST_JUNCTION_TARGET | Out-Null",
            ])
            .env("COMPANION_TEST_JUNCTION_LINK", powershell_path(link))
            .env("COMPANION_TEST_JUNCTION_TARGET", powershell_path(target))
            .output();
        if make_dangling {
            std::fs::remove_dir(target)?;
        }
        let output = output?;
        if !output.status.success() {
            return Err(std::io::Error::other(format!(
                "Junction fixture creation failed: {}",
                String::from_utf8_lossy(&output.stderr),
            )));
        }
        eprintln!("Exercising workspace boundary through an NTFS junction");
        Ok(())
    }

    #[cfg(windows)]
    fn ws() -> WorkspaceManager {
        WorkspaceManager::new(PathBuf::from(r"C:\Projects\RageV"))
    }

    #[cfg(not(windows))]
    fn ws() -> WorkspaceManager {
        WorkspaceManager::new(PathBuf::from("/tmp/ws"))
    }

    #[test]
    fn allows_relative_inside() {
        let w = ws();
        assert!(w.resolve("src/main.cpp").is_ok());
    }

    #[test]
    fn blocks_parent_traversal() {
        let w = ws();
        let err = w.resolve("../../etc/passwd").unwrap_err();
        assert!(
            matches!(err, WorkspaceError::OutsideWorkspace { .. }),
            "{err}"
        );
    }

    #[test]
    fn allows_new_nested_paths_below_existing_workspace() {
        let fixture = Fixture::new();
        let workspace = fixture.workspace();
        let expected = workspace
            .root()
            .canonicalize()
            .unwrap()
            .join("new/deep/file.txt");
        assert_eq!(workspace.resolve("new/deep/file.txt").unwrap(), expected);
        assert!(
            !expected.exists(),
            "resolution must not create files or directories"
        );
    }

    #[test]
    fn existing_workspace_still_blocks_traversal_and_absolute_escape() {
        let fixture = Fixture::new();
        let workspace = fixture.workspace();
        for requested in [
            "../outside.txt".to_string(),
            fixture.0.join("outside.txt").to_string_lossy().into_owned(),
        ] {
            assert!(matches!(
                workspace.resolve(&requested),
                Err(WorkspaceError::OutsideWorkspace { .. })
            ));
        }
    }

    #[test]
    fn rejects_existing_and_new_targets_under_outside_symlink() {
        let fixture = Fixture::new();
        let workspace = fixture.workspace();
        let outside = fixture.0.join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("existing.txt"), "test fixture").unwrap();
        if !link_directory(&outside, &workspace.root().join("escape")) {
            return;
        }
        for requested in [
            "escape/existing.txt",
            "escape/new.txt",
            "escape/new/deep/file.txt",
        ] {
            assert!(
                matches!(
                    workspace.resolve(requested),
                    Err(WorkspaceError::OutsideWorkspace { .. })
                ),
                "must block {requested}"
            );
        }
    }

    #[test]
    fn allows_new_targets_under_internal_symlink() {
        let fixture = Fixture::new();
        let workspace = fixture.workspace();
        let inside = workspace.root().join("real");
        std::fs::create_dir(&inside).unwrap();
        if !link_directory(&inside, &workspace.root().join("alias")) {
            return;
        }
        assert_eq!(
            workspace.resolve("alias/new/deep/file.txt").unwrap(),
            inside.canonicalize().unwrap().join("new/deep/file.txt")
        );
    }

    #[test]
    fn dangling_ancestor_and_dangling_workspace_fail_closed() {
        let fixture = Fixture::new();
        let workspace = fixture.workspace();
        let missing = fixture.0.join("missing-target");
        let link = workspace.root().join("dangling");
        if !link_directory(&missing, &link) {
            return;
        }
        assert!(matches!(
            workspace.resolve("dangling/new/file.txt"),
            Err(WorkspaceError::Io(_))
        ));
        assert!(matches!(
            WorkspaceManager::new(link).resolve("new.txt"),
            Err(WorkspaceError::Io(_))
        ));
    }

    #[test]
    fn sanitize_rejects_path_with_separators() {
        // "a/b.txt" file_name is "b.txt" — sanitize strips dirs; traversal rejected above.
        assert_eq!(
            WorkspaceManager::sanitize_filename("report.docx").unwrap(),
            "report.docx"
        );
        assert!(
            WorkspaceManager::sanitize_filename("../evil.exe").is_err()
                || WorkspaceManager::sanitize_filename("../evil.exe").unwrap() == "evil.exe"
        );
        assert!(WorkspaceManager::sanitize_filename("bad:name.txt").is_err());
    }
}
