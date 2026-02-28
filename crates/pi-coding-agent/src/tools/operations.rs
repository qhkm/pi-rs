use async_trait::async_trait;
use std::path::Path;

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
        tokio::fs::metadata(path).await.map(|m| m.is_dir()).unwrap_or(false)
    }
    async fn mkdir_p(&self, path: &Path) -> std::io::Result<()> {
        tokio::fs::create_dir_all(path).await
    }
}
