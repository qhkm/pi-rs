use async_trait::async_trait;
use std::path::{Path, PathBuf};

/// Sensitive system directories that must never be accessed, even via absolute paths.
const BLOCKED_PREFIXES: &[&str] = &[
    "/etc",
    "/var",
    "/usr",
    "/System",
    "/bin",
    "/sbin",
    "/private/etc",
    "/private/var",
];

/// Resolve `path` relative to `cwd` and validate that the result does not
/// escape the project root or point at sensitive system directories.
///
/// Rules:
/// - Relative paths are resolved against `cwd`.
/// - The resolved (canonicalized) path must start with `cwd` (the project root).
/// - Absolute paths that lie outside `cwd` are additionally checked against a
///   deny-list of sensitive system directories (`/etc`, `/var`, `/usr`, …).
/// - Symlinks are followed during canonicalization so that symlink traversal
///   attacks are caught just like `../..` traversal attacks.
/// - For paths that do not yet exist (e.g. a file about to be created), we
///   canonicalize the longest existing ancestor and reconstruct the suffix so
///   the check still works correctly.
///
/// Returns the validated absolute `PathBuf` on success or a human-readable
/// error string on rejection.
pub fn resolve_and_validate_path(cwd: &str, path: &str) -> Result<PathBuf, String> {
    let raw = if PathBuf::from(path).is_absolute() {
        PathBuf::from(path)
    } else {
        PathBuf::from(cwd).join(path)
    };

    // Canonicalize the cwd itself so we have a trustworthy anchor.
    let canonical_cwd = std::fs::canonicalize(cwd)
        .map_err(|e| format!("cannot canonicalize cwd '{}': {}", cwd, e))?;

    // Canonicalize the target, handling the case where the tail does not exist
    // yet (e.g. a new file that will be written).
    let canonical_target = canonicalize_non_existing(&raw)?;

    // ---- Check 1: path must be within cwd ----
    if canonical_target.starts_with(&canonical_cwd) {
        return Ok(canonical_target);
    }

    // ---- Check 2: absolute paths outside cwd are allowed only if they don't
    //      hit a blocked system prefix ----
    for blocked in BLOCKED_PREFIXES {
        let blocked_path = Path::new(blocked);
        if canonical_target.starts_with(blocked_path) {
            return Err(format!(
                "path traversal denied: '{}' resolves to '{}' which is inside a protected \
                 system directory ('{}').",
                path,
                canonical_target.display(),
                blocked,
            ));
        }
    }

    // The path is absolute, outside cwd, and not in a blocked system dir –
    // allow it (e.g. /tmp or a user-owned directory outside the project).
    Ok(canonical_target)
}

/// Canonicalize a path that may not fully exist by walking up to the first
/// existing ancestor, canonicalizing that, then appending the remaining suffix.
fn canonicalize_non_existing(path: &Path) -> Result<PathBuf, String> {
    // Fast path: the path already exists.
    if let Ok(c) = std::fs::canonicalize(path) {
        return Ok(c);
    }

    // Walk up until we find an existing ancestor.
    let mut existing = path.to_path_buf();
    let mut suffix = std::collections::VecDeque::new();

    loop {
        if existing.as_os_str().is_empty() || existing == Path::new("/") {
            break;
        }
        if existing.exists() {
            break;
        }
        if let Some(name) = existing.file_name() {
            suffix.push_front(name.to_owned());
        }
        match existing.parent() {
            Some(p) => existing = p.to_path_buf(),
            None => break,
        }
    }

    let mut base = std::fs::canonicalize(&existing)
        .map_err(|e| format!("cannot resolve path '{}': {}", path.display(), e))?;

    for component in suffix {
        base.push(component);
    }

    Ok(base)
}

/// Pluggable filesystem operations - implement for SSH/remote execution.
/// Default implementations use the local filesystem.
#[async_trait]
pub trait FileOperations: Send + Sync {
    async fn read_file(&self, path: &Path) -> std::io::Result<Vec<u8>>;
    async fn write_file(&self, path: &Path, content: &[u8]) -> std::io::Result<()>;
    async fn file_exists(&self, path: &Path) -> bool;
    async fn is_directory(&self, path: &Path) -> bool;
    async fn mkdir_p(&self, path: &Path) -> std::io::Result<()>;
}

/// Local filesystem implementation
pub struct LocalFileOps;

#[async_trait]
impl FileOperations for LocalFileOps {
    async fn read_file(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        tokio::fs::read(path).await
    }
    async fn write_file(&self, path: &Path, content: &[u8]) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(path, content).await
    }
    async fn file_exists(&self, path: &Path) -> bool {
        tokio::fs::metadata(path).await.is_ok()
    }
    async fn is_directory(&self, path: &Path) -> bool {
        tokio::fs::metadata(path)
            .await
            .map(|m| m.is_dir())
            .unwrap_or(false)
    }
    async fn mkdir_p(&self, path: &Path) -> std::io::Result<()> {
        tokio::fs::create_dir_all(path).await
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_and_validate_path;
    use std::fs;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Create a temporary directory that acts as the project root.
    fn mk_project() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    // -----------------------------------------------------------------------
    // Happy-path tests
    // -----------------------------------------------------------------------

    #[test]
    fn relative_path_within_project_resolves_correctly() {
        let project = mk_project();
        let cwd = project.path().to_str().unwrap();

        // Create a real file so canonicalize succeeds.
        let file = project.path().join("src/main.rs");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"fn main() {}").unwrap();

        let result = resolve_and_validate_path(cwd, "src/main.rs");
        assert!(result.is_ok(), "expected Ok, got: {:?}", result);
        assert_eq!(result.unwrap(), fs::canonicalize(&file).unwrap());
    }

    #[test]
    fn non_existing_file_within_project_is_allowed() {
        let project = mk_project();
        let cwd = project.path().to_str().unwrap();

        // The file does not exist yet (e.g. about to be written).
        let result = resolve_and_validate_path(cwd, "new_file.txt");
        assert!(
            result.is_ok(),
            "non-existing file inside project should be allowed, got: {:?}",
            result
        );
    }

    #[test]
    fn absolute_path_within_project_is_allowed() {
        let project = mk_project();
        let cwd = project.path().to_str().unwrap();

        let file = project.path().join("README.md");
        fs::write(&file, b"# readme").unwrap();
        let abs = file.to_str().unwrap();

        let result = resolve_and_validate_path(cwd, abs);
        assert!(
            result.is_ok(),
            "absolute path inside project should be allowed: {:?}",
            result
        );
    }

    // -----------------------------------------------------------------------
    // Traversal-attack tests
    // -----------------------------------------------------------------------

    #[test]
    fn dotdot_traversal_to_etc_passwd_is_rejected() {
        let project = mk_project();
        let cwd = project.path().to_str().unwrap();

        // Construct a path like <project>/../../etc/passwd
        let depth = project.path().components().count();
        let ups: String = "../".repeat(depth + 2);
        let malicious = format!("{}etc/passwd", ups);

        let result = resolve_and_validate_path(cwd, &malicious);
        assert!(
            result.is_err(),
            "expected Err for dotdot traversal, got Ok({:?})",
            result.ok()
        );
        let msg = result.unwrap_err();
        assert!(
            msg.contains("protected system directory") || msg.contains("traversal"),
            "error message should mention protection: {msg}"
        );
    }

    #[test]
    fn absolute_etc_shadow_is_rejected() {
        let project = mk_project();
        let cwd = project.path().to_str().unwrap();

        let result = resolve_and_validate_path(cwd, "/etc/shadow");
        assert!(
            result.is_err(),
            "expected Err for /etc/shadow, got Ok({:?})",
            result.ok()
        );
        let msg = result.unwrap_err();
        assert!(
            msg.contains("protected system directory"),
            "error message should mention protection: {msg}"
        );
    }

    #[test]
    fn absolute_usr_bin_is_rejected() {
        let project = mk_project();
        let cwd = project.path().to_str().unwrap();

        let result = resolve_and_validate_path(cwd, "/usr/bin/env");
        assert!(
            result.is_err(),
            "expected Err for /usr/bin/env, got Ok({:?})",
            result.ok()
        );
    }

    #[test]
    fn absolute_var_is_rejected() {
        let project = mk_project();
        let cwd = project.path().to_str().unwrap();

        let result = resolve_and_validate_path(cwd, "/var/log/system.log");
        assert!(
            result.is_err(),
            "expected Err for /var/log, got Ok({:?})",
            result.ok()
        );
    }

    // -----------------------------------------------------------------------
    // Symlink traversal test
    // -----------------------------------------------------------------------

    #[test]
    fn symlink_pointing_outside_cwd_is_rejected() {
        let project = mk_project();
        let cwd = project.path().to_str().unwrap();

        // Create a symlink inside the project that points at /etc.
        let link_path = project.path().join("evil_link");
        // Only run this test if /etc exists (it always does on macOS/Linux).
        if Path::new("/etc").exists() {
            symlink("/etc", &link_path).expect("symlink");

            let result = resolve_and_validate_path(cwd, "evil_link");
            assert!(
                result.is_err(),
                "symlink pointing to /etc should be rejected, got Ok({:?})",
                result.ok()
            );
        }
    }

    // Need Path in the symlink test above.
    use std::path::Path;
}
