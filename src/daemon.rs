use crate::registry::Registry;
use crate::server::{Hub, Server};
use anyhow::{Context, Result, bail};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{SignalKind, signal};
use tracing::{info, warn};

struct SocketGuard {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| metadata.dev() == self.device && metadata.ino() == self.inode)
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub fn socket_path() -> Result<PathBuf> {
    Ok(dirs::runtime_dir()
        .context("Could not determine user runtime directory")?
        .join("mcpd.sock"))
}

async fn remove_stale_socket(path: &Path) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_socket() {
        bail!("Refusing to replace non-socket path {}", path.display());
    }
    match UnixStream::connect(path).await {
        Ok(_) => bail!("mcpd daemon is already running at {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to inspect daemon socket {}", path.display()));
        }
    }
    std::fs::remove_file(path)
        .with_context(|| format!("Failed to remove stale socket {}", path.display()))?;
    Ok(())
}

pub async fn run(registry: Registry) -> Result<()> {
    let path = socket_path()?;
    remove_stale_socket(&path).await?;
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("Failed to bind daemon socket {}", path.display()))?;
    let metadata = std::fs::symlink_metadata(&path)?;
    let _guard = SocketGuard {
        path: path.clone(),
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let hub = Arc::new(Hub::new(registry));
    let mut connections = tokio::task::JoinSet::new();
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    info!(socket = %path.display(), "MCP daemon listening");

    let result = loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(connection) => connection,
                    Err(error) => break Err(error.into()),
                };
                let (input, output) = stream.into_split();
                let server = Server::from_hub_with_output(Arc::clone(&hub), output);
                connections.spawn(server.run_with(input));
            }
            completed = connections.join_next(), if !connections.is_empty() => {
                if let Some(Ok(Err(error))) = completed {
                    warn!(%error, "Client connection failed");
                }
            }
            _ = interrupt.recv() => break Ok(()),
            _ = terminate.recv() => break Ok(()),
        }
    };

    connections.abort_all();
    while connections.join_next().await.is_some() {}
    hub.shutdown().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stale_socket_is_removed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mcpd.sock");
        drop(UnixListener::bind(&path).unwrap());

        remove_stale_socket(&path).await.unwrap();

        assert!(!path.exists());
    }

    #[tokio::test]
    async fn live_socket_is_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mcpd.sock");
        let _listener = UnixListener::bind(&path).unwrap();

        let error = remove_stale_socket(&path).await.unwrap_err();

        assert!(error.to_string().contains("already running"));
        assert!(path.exists());
    }

    #[tokio::test]
    async fn non_socket_path_is_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mcpd.sock");
        std::fs::write(&path, "not a socket").unwrap();

        let error = remove_stale_socket(&path).await.unwrap_err();

        assert!(error.to_string().contains("non-socket"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "not a socket");
    }
}
