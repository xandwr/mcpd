use crate::daemon;
use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncWriteExt, copy};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_DELAY: Duration = Duration::from_millis(20);

fn can_start_daemon(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    )
}

async fn connect_after_start(path: &Path, child: &mut Child) -> Result<UnixStream> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    let mut status = None;
    loop {
        match UnixStream::connect(path).await {
            Ok(stream) => return Ok(stream),
            Err(_) if Instant::now() < deadline => {
                if status.is_none() {
                    status = child.try_wait()?;
                }
                tokio::time::sleep(RETRY_DELAY).await;
            }
            Err(error) => {
                if let Some(status) = status.or(child.try_wait()?) {
                    bail!(
                        "mcpd daemon exited with status {status} before its socket became available: {error}"
                    );
                }
                return Err(error).with_context(|| {
                    format!("Timed out waiting for daemon socket {}", path.display())
                });
            }
        }
    }
}

async fn connect_or_start(path: &Path) -> Result<(UnixStream, Option<Child>)> {
    match UnixStream::connect(path).await {
        Ok(stream) => return Ok((stream, None)),
        Err(error) if can_start_daemon(&error) => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to connect to daemon socket {}", path.display()));
        }
    }

    let executable = std::env::current_exe().context("Could not locate the mcpd executable")?;
    let mut child = Command::new(executable)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .context("Failed to start mcpd daemon")?;
    let stream = connect_after_start(path, &mut child).await?;
    Ok((stream, Some(child)))
}

pub async fn run() -> Result<()> {
    let path = daemon::socket_path()?;
    let (stream, child) = connect_or_start(&path).await?;
    if let Some(mut child) = child {
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
    }
    let (mut socket_input, mut socket_output) = stream.into_split();
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let upload = async {
        copy(&mut stdin, &mut socket_output).await?;
        socket_output.shutdown().await
    };
    let download = async {
        copy(&mut socket_input, &mut stdout).await?;
        stdout.flush().await
    };
    tokio::pin!(upload, download);
    tokio::select! {
        result = &mut upload => {
            result?;
            download.await?;
        }
        result = &mut download => result?,
    }
    Ok(())
}
