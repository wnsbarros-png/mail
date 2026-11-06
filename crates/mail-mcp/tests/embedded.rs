//! All accounts, messages and files here are synthetic; no UI-ready signal is
//! sent and no real mailbox/keychain is accessed.
use flectar_mail_core::{Core, accounts::credentials::UnavailableCredentialStore, config::Paths};
use mail_mcp::{
    AiConfig, Server,
    bridge::{BridgeClient, BridgeHost, ProfileGuard},
};
use serde_json::{Value, json};
use std::sync::Arc;

async fn fixture(config: AiConfig) -> (Server, Core, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::start_mail_ui_with_credentials(
        Paths::for_tests(dir.path()),
        Arc::new(UnavailableCredentialStore),
    )
    .await
    .unwrap();
    core.db.write(|conn| {
        conn.execute_batch("INSERT INTO accounts (id,email,provider,auth_kind,username,incoming_host,incoming_port,smtp_host,smtp_port,created_at,mail_protocol,settings_json) VALUES (1,'me@example.invalid','generic','password','me','invalid',993,'invalid',587,0,'imap','{\"connection\":{\"incomingSecurity\":\"tls\",\"smtpSecurity\":\"tls\"}}'); INSERT INTO sender_identities (account_id,email,is_primary,is_provider_default) VALUES (1,'me@example.invalid',1,1);")?;
        Ok(())
    }).await.unwrap();
    (Server::embedded(core.clone(), config), core, dir)
}

fn send_args(path: &std::path::Path) -> Value {
    json!({"accountId":1,"to":["recipient@example.invalid"],"subject":"Synthetic review","body":"Test only", "attachments":[path]})
}

#[tokio::test]
async fn bridge_reuses_engine_and_enforces_desktop_approval() {
    let (server, _core, dir) = fixture(AiConfig::default()).await;
    let _lease = ProfileGuard::acquire(dir.path()).unwrap();
    assert!(ProfileGuard::acquire(dir.path()).is_err());
    let host = BridgeHost::start(dir.path(), server.clone()).await.unwrap();
    let mut client = BridgeClient::connect(dir.path()).await.unwrap();
    assert!(
        client
            .handle(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await
            .unwrap()
            .is_none()
    );
    let reply = client
        .handle(
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_accounts"}}),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        reply["result"]["structuredContent"]["result"][0]["email"],
        "me@example.invalid"
    );
    let staged = server
        .call_tool("delete_message", &json!({"messageIds":[42]}))
        .await
        .unwrap();
    let id = staged["pendingId"].as_str().unwrap();
    assert!(
        server
            .call_tool("confirm_action", &json!({"pendingId":id}))
            .await
            .unwrap_err()
            .to_string()
            .contains("desktop app")
    );
    assert_eq!(server.pending_actions().await.len(), 1);
    server.cancel_in_app(id).await.unwrap();
    assert!(server.pending_actions().await.is_empty());
    assert!(server.approve_in_app(id).await.is_err());
    drop(host);
    assert!(!dir.path().join("ai-mcp-bridge.json").exists());
}

#[tokio::test]
async fn staged_attachments_are_copied_and_cancelled() {
    let (server, core, dir) = fixture(AiConfig::default()).await;
    let path = dir.path().join("report.txt");
    tokio::fs::write(&path, "original attachment")
        .await
        .unwrap();
    let staged = server
        .call_tool("send_message", &send_args(&path))
        .await
        .unwrap();
    assert_eq!(
        staged["summary"]["attachments"][0]["mimeType"],
        "text/plain"
    );
    let draft_id = staged["draftId"].as_i64().unwrap();
    let draft = core.get_draft(draft_id).await.unwrap();
    assert_eq!(draft.attachments.len(), 1);
    assert_ne!(std::path::Path::new(&draft.attachments[0].file_path), path);
    tokio::fs::write(&path, "modified source").await.unwrap();
    assert_eq!(
        tokio::fs::read_to_string(&draft.attachments[0].file_path)
            .await
            .unwrap(),
        "original attachment"
    );
    server
        .cancel_in_app(staged["pendingId"].as_str().unwrap())
        .await
        .unwrap();
    assert!(core.get_draft(draft_id).await.is_err());
}

#[tokio::test]
async fn only_ui_can_approve_and_only_once() {
    let (server, core, dir) = fixture(AiConfig::default()).await;
    let path = dir.path().join("report.pdf");
    tokio::fs::write(&path, b"synthetic").await.unwrap();
    let staged = server
        .call_tool("send_message", &send_args(&path))
        .await
        .unwrap();
    let id = staged["pendingId"].as_str().unwrap();
    let result = server.approve_in_app(id).await.unwrap();
    assert_eq!(result["status"], "queued");
    assert!(server.approve_in_app(id).await.is_err());
    assert_eq!(
        core.get_draft(staged["draftId"].as_i64().unwrap())
            .await
            .unwrap()
            .attachments
            .len(),
        1
    );
}

#[tokio::test]
async fn changed_draft_cannot_use_stale_approval() {
    let (server, core, dir) = fixture(AiConfig::default()).await;
    let path = dir.path().join("report.txt");
    tokio::fs::write(&path, b"test").await.unwrap();
    let staged = server
        .call_tool("send_message", &send_args(&path))
        .await
        .unwrap();
    let mut draft = core
        .get_draft(staged["draftId"].as_i64().unwrap())
        .await
        .unwrap();
    draft.subject = "Changed subject".into();
    core.save_draft(draft).await.unwrap();
    assert!(
        server
            .approve_in_app(staged["pendingId"].as_str().unwrap())
            .await
            .unwrap_err()
            .to_string()
            .contains("Draft changed")
    );
}

#[tokio::test]
async fn opt_in_mcp_confirmation_and_forced_desktop_staging() {
    let config = AiConfig {
        allow_mcp_confirmation: true,
        ai_actions_require_confirmation: false,
        ..Default::default()
    };
    let (server, _core, dir) = fixture(config).await;
    assert!(server.config().ai_actions_require_confirmation);
    let path = dir.path().join("report.txt");
    tokio::fs::write(&path, "test").await.unwrap();
    let staged = server
        .call_tool("send_message", &send_args(&path))
        .await
        .unwrap();
    let result = server
        .call_tool("confirm_action", &json!({"pendingId":staged["pendingId"]}))
        .await
        .unwrap();
    assert_eq!(result["status"], "queued");
}

#[tokio::test]
async fn untrusted_endpoint_and_wrong_token_are_rejected() {
    let (server, _core, dir) = fixture(AiConfig::default()).await;
    let _host = BridgeHost::start(dir.path(), server).await.unwrap();
    let path = dir.path().join("ai-mcp-bridge.json");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let mut endpoint: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    endpoint["token"] = json!("wrong token");
    std::fs::write(&path, endpoint.to_string()).unwrap();
    assert!(BridgeClient::connect(dir.path()).await.is_err());
    endpoint["address"] = json!("192.0.2.1:8000");
    std::fs::write(&path, endpoint.to_string()).unwrap();
    assert!(BridgeClient::connect(dir.path()).await.is_err());
}

#[tokio::test]
async fn stdio_binary_uses_live_desktop_despite_profile_lock() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (server, _core, dir) = fixture(AiConfig::default()).await;
    let _lease = ProfileGuard::acquire(dir.path()).unwrap();
    let _host = BridgeHost::start(dir.path(), server).await.unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mail-mcp"))
        .arg("--data-dir")
        .arg(dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"tools/call\",\"params\":{\"name\":\"list_accounts\"}}\n").await.unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(10), stdout.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let reply: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(reply["id"], 9);
    assert_eq!(
        reply["result"]["structuredContent"]["result"][0]["email"],
        "me@example.invalid"
    );
    drop(stdin);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn download_tool_checks_ownership_filename_and_no_overwrite() {
    let (server, core, dir) = fixture(AiConfig::default()).await;
    let source = dir.path().join("cached.txt");
    tokio::fs::write(&source, b"download fixture")
        .await
        .unwrap();
    let staged = server
        .call_tool("send_message", &send_args(&source))
        .await
        .unwrap();
    let message_id = staged["draftId"].as_i64().unwrap();
    let source_string = source.to_str().unwrap().to_string();
    let attachment_id = core.db.write(move |conn| {
        conn.execute("INSERT INTO attachments (message_id, filename, mime_type, size, file_path) VALUES (?1, 'document.txt', 'text/plain', 16, ?2)", (message_id, source_string))?;
        Ok(conn.last_insert_rowid())
    }).await.unwrap();
    let args = json!({"messageId":message_id,"attachmentId":attachment_id,"folder":dir.path()});
    let saved = server
        .call_tool("download_attachment", &args)
        .await
        .unwrap();
    assert_eq!(
        tokio::fs::read(saved["path"].as_str().unwrap())
            .await
            .unwrap(),
        b"download fixture"
    );
    assert!(
        server
            .call_tool("download_attachment", &args)
            .await
            .is_err()
    );
    let mut bad = args.clone();
    bad["filename"] = json!("../escape.txt");
    assert!(server.call_tool("download_attachment", &bad).await.is_err());
    bad["filename"] = json!("other.txt");
    bad["attachmentId"] = json!(attachment_id + 99);
    assert!(
        server
            .call_tool("download_attachment", &bad)
            .await
            .unwrap_err()
            .to_string()
            .contains("does not belong")
    );
}

#[tokio::test]
async fn unlocked_profile_ignores_stale_discovery_without_leaking_token() {
    use tokio::io::AsyncWriteExt;
    let dir = tempfile::tempdir().unwrap();
    let fake = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    std::fs::write(
        dir.path().join("ai-mcp-bridge.json"),
        json!({
            "address":fake.local_addr().unwrap(),"token":"stale-token-must-not-be-sent"
        })
        .to_string(),
    )
    .unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mail-mcp"))
        .arg("--data-dir")
        .arg(dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"list_accounts\"}}\n").await.unwrap();
    drop(stdin);
    let output = tokio::time::timeout(std::time::Duration::from_secs(15), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["result"]["structuredContent"]["result"], json!([]));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), fake.accept())
            .await
            .is_err()
    );
}
