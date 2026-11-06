//! Authenticated, loopback-only desktop transport. No HTTP/CORS surface.
//! Discovery is profile-local. A shared advisory lock is acquired *before*
//! either host opens the database, preventing competing sync engines.
use crate::{Server, branding};
use anyhow::{Context, Result, bail};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    task::{JoinHandle, JoinSet},
};

const DISCOVERY: &str = "ai-mcp-bridge.json";
const MAX_FRAME: usize = 2 * 1024 * 1024;

pub struct ProfileGuard {
    _file: File,
}
impl ProfileGuard {
    pub fn acquire(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(data_dir.join(branding::LOCK_FILE))?;
        file.try_lock().context("This profile is already in use. If the desktop app is running, use its MCP bridge; otherwise stop the standalone mail-mcp session before opening the app.")?;
        Ok(Self { _file: file })
    }
}

#[derive(Serialize, Deserialize)]
struct Endpoint {
    address: SocketAddr,
    token: String,
}

/// Owns discovery and listener lifetime. Drop aborts all active connections.
pub struct BridgeHost {
    task: JoinHandle<()>,
    path: PathBuf,
}
impl Drop for BridgeHost {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Bounded framing also protects against a local client that never sends a newline.
async fn read_frame<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<String>> {
    let mut bytes = Vec::new();
    let n = reader
        .take((MAX_FRAME + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .await?;
    if n == 0 {
        return Ok(None);
    }
    if n > MAX_FRAME || bytes.last() != Some(&b'\n') {
        bail!("invalid or oversized bridge frame");
    }
    Ok(Some(String::from_utf8(bytes)?))
}

impl BridgeHost {
    pub async fn start(data_dir: &Path, server: Server) -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let token = format!(
            "{:032x}{:032x}",
            rand::rng().random::<u128>(),
            rand::rng().random::<u128>()
        ); // 256 random bits
        let endpoint = Endpoint {
            address: listener.local_addr()?,
            token: token.clone(),
        };
        let path = data_dir.join(DISCOVERY);
        // Replacing a stale endpoint never follows a symlink. The profile lock
        // ensures that no live host is publishing to the same path.
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        use std::io::Write;
        file.write_all(&serde_json::to_vec(&endpoint)?)?;
        let token: Arc<str> = token.into();
        let task = tokio::spawn(async move {
            let mut clients = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, peer)) = accepted else { break; };
                        if !peer.ip().is_loopback() || clients.len() >= 16 { continue; }
                        let server = server.clone(); let token = token.clone();
                        clients.spawn(async move {
                            if let Err(error) = serve(stream, &token, server).await {
                                tracing::debug!(%error, "MCP bridge connection closed");
                            }
                        });
                    }
                    _ = clients.join_next(), if !clients.is_empty() => {}
                }
            }
        });
        Ok(Self { task, path })
    }
}

async fn serve(stream: TcpStream, token: &str, server: Server) -> Result<()> {
    let mut stream = BufReader::new(stream);
    let auth = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut stream))
        .await??
        .context("missing authentication")?;
    let auth: Value = serde_json::from_str(&auth)?;
    if auth.get("token").and_then(Value::as_str) != Some(token) {
        bail!("unauthorized bridge client");
    }
    stream
        .get_mut()
        .write_all(b"{\"authenticated\":true}\n")
        .await?;
    while let Some(line) = read_frame(&mut stream).await? {
        let msg: Value = serde_json::from_str(&line)?;
        // Envelope always has a response, including for MCP notifications.
        let reply = json!({"reply": server.handle(msg).await});
        stream
            .get_mut()
            .write_all(reply.to_string().as_bytes())
            .await?;
        stream.get_mut().write_all(b"\n").await?;
    }
    Ok(())
}

pub struct BridgeClient {
    stream: BufReader<TcpStream>,
}
impl BridgeClient {
    /// No database access. A stale or invalid endpoint is an error; the caller
    /// may only fall back after acquiring ProfileGuard successfully.
    pub async fn connect(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join(DISCOVERY);
        let text = tokio::fs::read(path).await?;
        if text.len() > 4096 {
            bail!("invalid bridge discovery file");
        }
        let endpoint: Endpoint = serde_json::from_slice(&text)?;
        if endpoint.address.ip() != std::net::IpAddr::V4(Ipv4Addr::LOCALHOST) {
            bail!("bridge endpoint must be 127.0.0.1");
        }
        let stream =
            tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(endpoint.address))
                .await??;
        let mut client = Self {
            stream: BufReader::new(stream),
        };
        client
            .stream
            .get_mut()
            .write_all(format!("{}\n", json!({"token": endpoint.token})).as_bytes())
            .await?;
        let ack = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut client.stream))
            .await??
            .context("bridge authentication failed")?;
        if serde_json::from_str::<Value>(&ack)?["authenticated"] != true {
            bail!("bridge authentication failed");
        }
        Ok(client)
    }
    pub async fn handle(&mut self, msg: Value) -> Result<Option<Value>> {
        let line = format!("{msg}\n");
        if line.len() > MAX_FRAME {
            bail!("request exceeds bridge frame limit");
        }
        self.stream.get_mut().write_all(line.as_bytes()).await?;
        let reply = read_frame(&mut self.stream)
            .await?
            .context("desktop app disconnected; restart the MCP session")?;
        let value: Value = serde_json::from_str(&reply)?;
        Ok(value.get("reply").filter(|v| !v.is_null()).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_lock_releases_without_deleting_lock_file() {
        let dir = tempfile::tempdir().unwrap();
        let first = ProfileGuard::acquire(dir.path()).unwrap();
        assert!(ProfileGuard::acquire(dir.path()).is_err());
        drop(first);
        let _next = ProfileGuard::acquire(dir.path()).unwrap();
    }
    #[tokio::test]
    async fn bounded_framing_rejects_oversized_and_unterminated_input() {
        let input = vec![b'x'; MAX_FRAME + 1];
        assert!(
            read_frame(&mut BufReader::new(input.as_slice()))
                .await
                .is_err()
        );
        assert!(read_frame(&mut BufReader::new(&b"{}"[..])).await.is_err());
        assert_eq!(
            read_frame(&mut BufReader::new(&b"{}\n"[..])).await.unwrap(),
            Some("{}\n".into())
        );
        assert_eq!(
            read_frame(&mut BufReader::new(&b""[..])).await.unwrap(),
            None
        );
    }
}
