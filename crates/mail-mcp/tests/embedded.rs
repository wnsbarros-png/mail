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

#[derive(Clone)]
struct FakeStorage {
    calls: Arc<std::sync::Mutex<Vec<String>>>,
    fail_share: bool,
}
#[async_trait::async_trait]
impl flectar_mail_core::cloud_links::StorageProvider for FakeStorage {
    fn provider(&self) -> flectar_mail_core::cloud_links::CloudProvider {
        flectar_mail_core::cloud_links::CloudProvider::GoogleDrive
    }
    async fn upload(
        &self,
        file: &flectar_mail_core::cloud_links::CloudFile,
        progress: flectar_mail_core::cloud_links::Progress,
    ) -> flectar_mail_core::error::Result<flectar_mail_core::cloud_links::UploadedFile> {
        flectar_mail_core::cloud_links::verify(file).await?;
        let bytes = tokio::fs::read_to_string(&file.path).await?;
        self.calls
            .lock()
            .unwrap()
            .push(format!("upload:{}:{bytes}", file.name));
        progress(file.size, file.size);
        Ok(flectar_mail_core::cloud_links::UploadedFile {
            id: "NEW-ONLY".into(),
            name: file.name.clone(),
            size: file.size,
        })
    }
    async fn share(
        &self,
        file: &flectar_mail_core::cloud_links::UploadedFile,
        expiry: Option<&str>,
    ) -> flectar_mail_core::error::Result<flectar_mail_core::cloud_links::SharedFile> {
        self.calls.lock().unwrap().push("share:anyone:view".into());
        if self.fail_share {
            return Err(flectar_mail_core::error::CoreError::Other(
                "Synthetic permission denied".into(),
            ));
        }
        Ok(flectar_mail_core::cloud_links::SharedFile {
            id: file.id.clone(),
            name: file.name.clone(),
            size: file.size,
            provider: self.provider(),
            url: "https://example.invalid/download?synthetic=true".into(),
            expires_at: expiry.map(String::from),
        })
    }
    async fn delete(&self, id: &str) -> flectar_mail_core::error::Result<()> {
        self.calls.lock().unwrap().push(format!("delete:{id}"));
        Ok(())
    }
}
fn cloud_args(path: &std::path::Path) -> Value {
    let mut args = send_args(path);
    args["attachments_as_link"] = json!(true);
    args["provider"] = json!("google_drive");
    args
}
#[tokio::test]
async fn cloud_upload_requires_app_even_if_mcp_confirmation_setting_enabled() {
    let (server, core, dir) = fixture(AiConfig {
        allow_mcp_confirmation: true,
        ..Default::default()
    })
    .await;
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let server = server.with_storage_provider(Arc::new(FakeStorage {
        calls: calls.clone(),
        fail_share: false,
    }));
    let path = dir.path().join("large-report.txt");
    tokio::fs::write(&path, "approved-original").await.unwrap();
    let staged = server
        .call_tool("send_message", &cloud_args(&path))
        .await
        .unwrap();
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(staged["summary"]["provider"], "Google Drive");
    assert_eq!(staged["summary"]["anyoneWithLink"], true);
    let id = staged["pendingId"].as_str().unwrap();
    assert!(
        server
            .call_tool("confirm_action", &json!({"pendingId":id}))
            .await
            .is_err()
    );
    assert!(calls.lock().unwrap().is_empty());
    tokio::fs::write(&path, "unapproved-change").await.unwrap();
    let result = server.approve_in_app(id).await.unwrap();
    assert_eq!(result["status"], "queued");
    {
        let calls = calls.lock().unwrap();
        assert!(calls[0].ends_with(":approved-original"));
        assert_eq!(calls[1], "share:anyone:view");
    }
    let draft = core
        .get_draft(staged["draftId"].as_i64().unwrap())
        .await
        .unwrap();
    assert!(draft.attachments.is_empty());
    assert!(
        draft
            .body_text
            .contains("https://example.invalid/download?synthetic=true")
    );
    assert!(draft.body_html.unwrap().contains("href="));
    assert!(
        std::fs::read_dir(dir.path().join("cloud-pending"))
            .unwrap()
            .next()
            .is_none()
    );
}
#[tokio::test]
async fn cloud_cancel_removes_private_copies_without_upload() {
    let (server, _core, dir) = fixture(AiConfig::default()).await;
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let server = server.with_storage_provider(Arc::new(FakeStorage {
        calls: calls.clone(),
        fail_share: false,
    }));
    let path = dir.path().join("cancel.txt");
    tokio::fs::write(&path, "test").await.unwrap();
    let staged = server
        .call_tool("send_message", &cloud_args(&path))
        .await
        .unwrap();
    server
        .cancel_in_app(staged["pendingId"].as_str().unwrap())
        .await
        .unwrap();
    assert!(calls.lock().unwrap().is_empty());
    assert!(
        std::fs::read_dir(dir.path().join("cloud-pending"))
            .unwrap()
            .next()
            .is_none()
    );
}
#[tokio::test]
async fn cloud_permission_failure_never_queues_mail_and_cleans_only_new_files() {
    let (server, core, dir) = fixture(AiConfig::default()).await;
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let server = server.with_storage_provider(Arc::new(FakeStorage {
        calls: calls.clone(),
        fail_share: true,
    }));
    let path = dir.path().join("blocked.txt");
    tokio::fs::write(&path, "test").await.unwrap();
    let staged = server
        .call_tool("send_message", &cloud_args(&path))
        .await
        .unwrap();
    let error = server
        .approve_in_app(staged["pendingId"].as_str().unwrap())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("No email was queued"));
    assert_eq!(calls.lock().unwrap().last().unwrap(), "delete:NEW-ONLY");
    let count = core
        .db
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM pending_actions WHERE kind='send'",
                [],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(count, 0);
}
#[tokio::test]
async fn cloud_edited_draft_requires_new_review_without_upload() {
    let (server, core, dir) = fixture(AiConfig::default()).await;
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let server = server.with_storage_provider(Arc::new(FakeStorage {
        calls: calls.clone(),
        fail_share: false,
    }));
    let path = dir.path().join("draft.txt");
    tokio::fs::write(&path, "test").await.unwrap();
    let staged = server
        .call_tool("send_message", &cloud_args(&path))
        .await
        .unwrap();
    let mut draft = core
        .get_draft(staged["draftId"].as_i64().unwrap())
        .await
        .unwrap();
    draft.subject = "Unapproved".into();
    core.save_draft(draft).await.unwrap();
    assert!(
        server
            .approve_in_app(staged["pendingId"].as_str().unwrap())
            .await
            .is_err()
    );
    assert!(calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn account_limit_auto_fallback_is_explicitly_opt_in() {
    let (server, _core, dir) = fixture(AiConfig::default()).await;
    let path = dir.path().join("over.txt");
    tokio::fs::write(&path, "test").await.unwrap();
    let config = dir.path().join("cloud-links.json");
    tokio::fs::write(&config, r#"{"account_limits":{"1":2}}"#)
        .await
        .unwrap();
    assert!(
        server
            .call_tool("send_message", &send_args(&path))
            .await
            .is_err()
    );
    tokio::fs::write(
        &config,
        r#"{"account_limits":{"1":2},"auto_link_over_limit":true,"default_provider":"dropbox"}"#,
    )
    .await
    .unwrap();
    let staged = server
        .call_tool("send_message", &send_args(&path))
        .await
        .unwrap();
    assert_eq!(staged["summary"]["provider"], "Dropbox");
    assert_eq!(staged["status"], "pending_confirmation");
    server
        .cancel_in_app(staged["pendingId"].as_str().unwrap())
        .await
        .unwrap();
}
#[tokio::test]
async fn cloud_more_than_25_mib_stages_but_never_uploads_before_approval() {
    let (server, core, dir) = fixture(AiConfig::default()).await;
    let path = dir.path().join("large.bin");
    let file = tokio::fs::File::create(&path).await.unwrap();
    file.set_len(26 * 1024 * 1024).await.unwrap();
    drop(file);
    let staged = server
        .call_tool("send_message", &cloud_args(&path))
        .await
        .unwrap();
    assert_eq!(staged["summary"]["files"][0]["size"], 26 * 1024 * 1024);
    assert!(
        core.get_draft(staged["draftId"].as_i64().unwrap())
            .await
            .unwrap()
            .attachments
            .is_empty()
    );
    server
        .cancel_in_app(staged["pendingId"].as_str().unwrap())
        .await
        .unwrap();
}

#[tokio::test]
async fn reply_cloud_preserves_thread_quote_and_approval_gate() {
    use flectar_mail_core::models::{Address, SaveDraftArgs};
    let (server, core, dir) = fixture(AiConfig::default()).await;
    let original = core
        .save_draft(SaveDraftArgs {
            draft_id: None,
            account_id: 1,
            from: None,
            to: vec![Address {
                name: None,
                email: "recipient@example.invalid".into(),
            }],
            cc: vec![],
            bcc: vec![],
            subject: "Original subject".into(),
            body_text: "Original quoted body".into(),
            body_html: None,
            mode: "new".into(),
            in_reply_to_message_id: None,
            in_reply_to_header: None,
            attachments: vec![],
        })
        .await
        .unwrap();
    let path = dir.path().join("reply.txt");
    tokio::fs::write(&path, "synthetic reply file")
        .await
        .unwrap();
    let staged=server.call_tool("reply_message",&json!({"messageId":original,"body":"Reply with a link","attachments":[path],"attachments_as_link":true,"provider":"one_drive","storage_account_id":1})).await.unwrap();
    let draft = core
        .get_draft(staged["draftId"].as_i64().unwrap())
        .await
        .unwrap();
    assert_eq!(draft.in_reply_to_message_id, Some(original));
    assert_eq!(draft.subject, "Re: Original subject");
    assert!(draft.body_text.contains("Original quoted body"));
    assert_eq!(staged["summary"]["provider"], "OneDrive");
    server
        .cancel_in_app(staged["pendingId"].as_str().unwrap())
        .await
        .unwrap();
}
#[tokio::test]
async fn standalone_cannot_stage_or_approve_cloud_even_with_confirmation_disabled() {
    let (_embedded, core, dir) = fixture(AiConfig::default()).await;
    let server = Server::new(
        core,
        AiConfig {
            ai_actions_require_confirmation: false,
            allow_mcp_confirmation: true,
            ..Default::default()
        },
    );
    let path = dir.path().join("headless.txt");
    tokio::fs::write(&path, "test").await.unwrap();
    let error = server
        .call_tool("send_message", &cloud_args(&path))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("desktop app"));
    assert!(server.pending_actions().await.is_empty());
    assert!(!dir.path().join("cloud-pending").exists());
}
