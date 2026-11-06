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
    // One MCP engine per profile: a second one would run a second sync engine.
    let lock = std::fs::File::create(paths.data_dir.join(branding::LOCK_FILE))?;
    if lock.try_lock().is_err() {
        bail!(
            "another mail-mcp server is already running for {}",
            paths.data_dir.display()
        );
    }
    let config = AiConfig::load_or_init(&paths.data_dir);
    let core = Core::start_mail_ui(paths)
        .await
        .context("starting the mail core")?;
    // No window will report in; start account sync right away.
    core.notify_ui_ready();
    let server = Server::new(core, config);

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str(&line) {
            Ok(msg) => server.handle(msg).await,
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
    drop(lock);
    Ok(())
}
