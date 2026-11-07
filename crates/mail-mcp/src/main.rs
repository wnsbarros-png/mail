//! `mail-mcp`: stdio MCP server over the mail client's local profile.
//!
//! Usage: mail-mcp [--data-dir DIR] [--cache-dir DIR]
//! stdout carries only JSON-RPC; logs go to stderr (RUST_LOG, default warn).

use anyhow::{Context, Result, bail};
use flectar_mail_core::Core;
use flectar_mail_core::config::Paths;
use mail_mcp::{AiConfig, Server, branding};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn parse_paths() -> Result<Paths> {
    let mut data = None;
    let mut cache = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data-dir" => data = args.next().map(PathBuf::from),
            "--cache-dir" => cache = args.next().map(PathBuf::from),
            "-h" | "--help" => {
                eprintln!(
                    "{} MCP server\nUsage: mail-mcp [--data-dir DIR] [--cache-dir DIR]\nSpeaks MCP (JSON-RPC 2.0) on stdin/stdout. See docs/ai-mcp.md.",
                    branding::APP_NAME
                );
                std::process::exit(0);
            }
            "--version" => {
                println!("{}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => bail!("unknown argument `{other}` (try --help)"),
        }
    }
    let defaults = Paths::default_dirs().context("resolving the application data directory")?;
    let data_dir = data.unwrap_or(defaults.data_dir);
    let cache_dir = cache.unwrap_or_else(|| data_dir.join("cache"));
    Ok(Paths::new(data_dir, cache_dir))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let paths = parse_paths()?;
    paths.ensure().context("creating application directories")?;
    // Prefer the live desktop engine. If discovery races with startup,
    // wait briefly while the shared profile lock prevents a second engine.
    let (server, mut bridge, _lock) = match mail_mcp::bridge::ProfileGuard::acquire(&paths.data_dir)
    {
        Ok(lock) => {
            // An unlocked profile means there is no live desktop host.
            // Ignore stale discovery rather than sending a token to a port
            // that another process could have reused after a crash.
            let config = AiConfig::load_or_init(&paths.data_dir);
            let core = Core::start_mail_ui(paths.clone())
                .await
                .context("starting the mail core")?;
            core.notify_ui_ready();
            (Some(Server::new(core, config)), None, Some(lock))
        }
        Err(error) => {
            let mut client = None;
            for _ in 0..40 {
                if let Ok(connected) =
                    mail_mcp::bridge::BridgeClient::connect(&paths.data_dir).await
                {
                    client = Some(connected);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            (None, Some(client.ok_or(error)?), None)
        }
    };

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str(&line) {
            Ok(msg) => {
                if let Some(client) = bridge.as_mut() {
                    client.handle(msg).await?
                } else {
                    server
                        .as_ref()
                        .expect("standalone server")
                        .handle(msg)
                        .await
                }
            }
            Err(e) => Some(
                serde_json::json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":format!("parse error: {e}")}}),
            ),
        };
        if let Some(reply) = reply {
            stdout.write_all(reply.to_string().as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}
