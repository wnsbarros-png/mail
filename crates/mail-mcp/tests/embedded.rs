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
    let mut policy = mail_mcp::policy::Config::default();
    policy.defaults.insert(
        mail_mcp::policy::Action::Download,
        mail_mcp::policy::Effect::Allow,
    );
    server.save_policy(policy).await.unwrap();
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
    let downloads = tempfile::tempdir().unwrap();
    let args =
        json!({"messageId":message_id,"attachmentId":attachment_id,"folder":downloads.path()});
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

use mail_mcp::policy::{Action as PolicyAction, Conditions, Config as Policy, Effect, Rule};
async fn original(server: &Server, dir: &std::path::Path) -> i64 {
    let file = dir.join("PRIVATE_FILENAME.txt");
    tokio::fs::write(&file, "synthetic bytes").await.unwrap();
    let mut args = send_args(&file);
    args["body"] = json!("PRIVATE_BODY_SENTINEL");
    args["subject"] = json!("PRIVATE_SUBJECT_SENTINEL");
    server.call_tool("send_message", &args).await.unwrap()["draftId"]
        .as_i64()
        .unwrap()
}
#[tokio::test]
async fn every_exposed_tool_has_a_policy_classification() {
    let (server, _, _dir) = fixture(AiConfig::default()).await;
    for tool in server.tool_definitions().as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        assert!(PolicyAction::for_tool(name, &json!({})).is_some(), "{name}");
    }
}
#[tokio::test]
async fn read_ask_can_be_approved_and_retrieved_but_block_revokes_cache_access() {
    let (server, _, dir) = fixture(AiConfig::default()).await;
    let id = original(&server, dir.path()).await;
    let mut policy = Policy::default();
    policy.defaults.insert(PolicyAction::Read, Effect::Ask);
    server.save_policy(policy.clone()).await.unwrap();
    let pending = server
        .call_tool("read_message", &json!({"messageId":id}))
        .await
        .unwrap();
    assert!(!pending.to_string().contains("PRIVATE_BODY_SENTINEL"));
    let pending_id = pending["pendingId"].as_str().unwrap();
    assert_eq!(
        server
            .call_tool("get_action_result", &json!({"pendingId":pending_id}))
            .await
            .unwrap()["status"],
        "not_available"
    );
    let approved = server.approve_in_app(pending_id).await.unwrap();
    assert_eq!(approved["body"], "PRIVATE_BODY_SENTINEL");
    let output = server
        .call_tool("get_action_result", &json!({"pendingId":pending_id}))
        .await
        .unwrap();
    assert_eq!(output["result"]["body"], "PRIVATE_BODY_SENTINEL");
    assert_eq!(output["accessPolicy"]["effect"], "ask");
    policy.defaults.insert(PolicyAction::Read, Effect::Block);
    server.save_policy(policy).await.unwrap();
    let response=server.handle(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_action_result","arguments":{"pendingId":pending_id}}})).await.unwrap();
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        response["result"]["structuredContent"]["policy"]["effect"],
        "block"
    );
    assert!(!response.to_string().contains("PRIVATE_BODY_SENTINEL"));
}
#[tokio::test]
async fn policy_is_rechecked_before_approving_staged_send() {
    let (server, core, dir) = fixture(AiConfig::default()).await;
    let file = dir.path().join("test.txt");
    tokio::fs::write(&file, "test").await.unwrap();
    let pending = server
        .call_tool("send_message", &send_args(&file))
        .await
        .unwrap();
    let mut p = Policy::default();
    p.defaults.insert(PolicyAction::Send, Effect::Block);
    server.save_policy(p).await.unwrap();
    assert!(
        server
            .approve_in_app(pending["pendingId"].as_str().unwrap())
            .await
            .is_err()
    );
    assert_eq!(queued_sends(&core).await, 0);
    assert!(
        server
            .action_history()
            .await
            .unwrap()
            .iter()
            .any(|e| e.result == "blocked_at_approval")
    );
}
#[tokio::test]
async fn new_recipient_guard_ignores_incoming_spam_but_recognizes_outgoing_history() {
    let (server, core, dir) = fixture(AiConfig::default()).await;
    let file = dir.path().join("test.txt");
    tokio::fs::write(&file, "test").await.unwrap();
    let mut p = Policy::default();
    p.defaults.insert(PolicyAction::Send, Effect::Allow);
    server.save_policy(p).await.unwrap();
    core.db.write(|c|{c.execute("INSERT INTO contacts (email,recv_count,send_count) VALUES ('recipient@example.invalid',999,0)",[])?;Ok(())}).await.unwrap();
    let pending = server
        .call_tool("send_message", &send_args(&file))
        .await
        .unwrap();
    assert_eq!(pending["status"], "pending_confirmation");
    assert_eq!(
        pending["policy"]["reason"],
        "invariant.new_recipient_requires_approval"
    );
    server
        .cancel_in_app(pending["pendingId"].as_str().unwrap())
        .await
        .unwrap();
    core.db
        .write(|c| {
            c.execute("UPDATE contacts SET send_count=1", [])?;
            Ok(())
        })
        .await
        .unwrap();
    let sent = server
        .call_tool("send_message", &send_args(&file))
        .await
        .unwrap();
    assert_eq!(sent["status"], "queued");
    assert_eq!(queued_sends(&core).await, 1);
}
#[tokio::test]
async fn saved_contacts_condition_does_not_trust_learned_incoming_or_outgoing_contacts() {
    let (server, core, dir) = fixture(AiConfig::default()).await;
    let file = dir.path().join("test.txt");
    tokio::fs::write(&file, "test").await.unwrap();
    let mut p = Policy::default();
    p.rules.push(Rule {
        id: "saved-only".into(),
        action: PolicyAction::Send,
        effect: Effect::Allow,
        conditions: Conditions {
            known_contacts: Some(true),
            ..Default::default()
        },
        override_new_recipients: false,
    });
    server.save_policy(p).await.unwrap();
    core.db
        .write(|c| {
            c.execute(
                "INSERT INTO contacts (email,send_count) VALUES ('recipient@example.invalid',3)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let pending = server
        .call_tool("send_message", &send_args(&file))
        .await
        .unwrap();
    assert_eq!(pending["status"], "pending_confirmation");
    server
        .cancel_in_app(pending["pendingId"].as_str().unwrap())
        .await
        .unwrap();
    core.db.write(|c|{c.execute("INSERT INTO contact_entries (document,name,email,phone,company,job_title,website,birthday,postal_address,notes,tags,folded) VALUES ('{}','Recipient','recipient@example.invalid','','','','','','','','','')",[])?;let id=c.last_insert_rowid();c.execute("INSERT INTO contact_values (contact_id,line,kind,value,label,preference) VALUES (?1,1,'EMAIL','RECIPIENT@example.invalid','',0)",[id])?;Ok(())}).await.unwrap();
    let sent = server
        .call_tool("send_message", &send_args(&file))
        .await
        .unwrap();
    assert_eq!(sent["status"], "queued");
    assert_eq!(sent["policy"]["applied"][0]["rule_id"], "saved-only");
}
#[tokio::test]
async fn bcc_is_counted_and_allowlist_must_cover_every_recipient() {
    let (server, _core, dir) = fixture(AiConfig::default()).await;
    let file = dir.path().join("test.txt");
    tokio::fs::write(&file, "test").await.unwrap();
    let mut p = Policy::default();
    p.rules.push(Rule {
        id: "allow-one".into(),
        action: PolicyAction::Send,
        effect: Effect::Allow,
        conditions: Conditions {
            addresses: vec!["recipient@example.invalid".into()],
            max_recipients: Some(1),
            ..Default::default()
        },
        override_new_recipients: true,
    });
    server.save_policy(p).await.unwrap();
    let mut args = send_args(&file);
    args["bcc"] = json!(["hidden@other.invalid"]);
    let pending = server.call_tool("send_message", &args).await.unwrap();
    assert_eq!(pending["status"], "pending_confirmation");
    assert_eq!(pending["policy"]["applied"][0]["rule_id"], "default.send");
    let log = server.action_history().await.unwrap();
    let event = log.iter().find(|e| e.action == PolicyAction::Send).unwrap();
    assert_eq!(event.recipient_count, 2);
    assert_eq!(event.new_recipient_count, 2);
}
#[tokio::test]
async fn account_rule_cannot_be_bypassed_by_an_ignored_list_accounts_argument() {
    let (server, core, _dir) = fixture(AiConfig::default()).await;
    core.db.write(|c|{c.execute_batch("INSERT INTO accounts (id,email,provider,auth_kind,username,incoming_host,incoming_port,smtp_host,smtp_port,created_at,mail_protocol,settings_json) SELECT 2,'second@example.invalid',provider,auth_kind,username,incoming_host,incoming_port,smtp_host,smtp_port,created_at,mail_protocol,settings_json FROM accounts WHERE id=1")?;Ok(())}).await.unwrap();
    let mut p = Policy::default();
    p.rules.push(Rule {
        id: "private-account".into(),
        action: PolicyAction::List,
        effect: Effect::Block,
        conditions: Conditions {
            accounts: vec![2],
            ..Default::default()
        },
        override_new_recipients: false,
    });
    server.save_policy(p).await.unwrap();
    let err = server
        .call_tool("list_accounts", &json!({"accountId":1}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("private-account"));
    assert!(!err.to_string().contains("second@example.invalid"));
    let id = original(&server, _dir.path()).await;
    for args in [
        json!({"messageId":id,"accountId":1}),
        json!({"messageIds":[id],"accountId":1}),
    ] {
        assert!(
            server
                .call_tool("list_accounts", &args)
                .await
                .unwrap_err()
                .to_string()
                .contains("private-account")
        );
    }
}
#[tokio::test]
async fn cloud_always_asks_and_a_new_block_prevents_any_upload() {
    let (server, core, dir) = fixture(AiConfig::default()).await;
    let calls = Arc::new(std::sync::Mutex::new(vec![]));
    let server = server.with_storage_provider(Arc::new(FakeStorage {
        calls: calls.clone(),
        fail_share: false,
    }));
    let file = dir.path().join("test.txt");
    tokio::fs::write(&file, "test").await.unwrap();
    let mut p = Policy::default();
    p.defaults.insert(PolicyAction::Send, Effect::Allow);
    p.defaults.insert(PolicyAction::CloudUpload, Effect::Allow);
    server.save_policy(p.clone()).await.unwrap();
    let pending = server
        .call_tool("send_message", &cloud_args(&file))
        .await
        .unwrap();
    assert_eq!(pending["policy"]["effect"], "ask");
    assert_eq!(pending["policy"]["in_app_only"], true);
    assert!(calls.lock().unwrap().is_empty());
    p.defaults.insert(PolicyAction::CloudUpload, Effect::Block);
    server.save_policy(p).await.unwrap();
    assert!(
        server
            .approve_in_app(pending["pendingId"].as_str().unwrap())
            .await
            .is_err()
    );
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(queued_sends(&core).await, 0);
}
#[tokio::test]
async fn history_has_no_sensitive_payload_and_distinguishes_cancellation_actor() {
    let (server, _, dir) = fixture(AiConfig::default()).await;
    let _ = original(&server, dir.path()).await;
    let pending = server.pending_actions().await;
    server
        .cancel_in_app(pending[0]["pendingId"].as_str().unwrap())
        .await
        .unwrap();
    let events = server.action_history().await.unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.actor == "desktop_user" && e.result == "cancelled")
    );
    let bytes = std::fs::read_to_string(dir.path().join("ai-history.jsonl")).unwrap();
    for secret in [
        "PRIVATE_BODY_SENTINEL",
        "PRIVATE_SUBJECT_SENTINEL",
        "PRIVATE_FILENAME",
        "recipient@example.invalid",
    ] {
        assert!(!bytes.contains(secret));
    }
}
#[tokio::test]
async fn unwritable_history_fails_closed_before_a_draft_is_created() {
    let (server, core, dir) = fixture(AiConfig::default()).await;
    std::fs::create_dir(dir.path().join("ai-history.jsonl")).unwrap();
    let file = dir.path().join("test.txt");
    tokio::fs::write(&file, "test").await.unwrap();
    assert!(
        server
            .call_tool("send_message", &send_args(&file))
            .await
            .is_err()
    );
    assert!(server.pending_actions().await.is_empty());
    assert_eq!(
        core.db
            .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get::<_, i64>(0))?))
            .await
            .unwrap(),
        0
    );
}
#[tokio::test]
async fn mark_read_option_obeys_its_own_policy_and_reply_obeys_read_policy() {
    let (server, _, dir) = fixture(AiConfig::default()).await;
    let id = original(&server, dir.path()).await;
    let mut p = Policy::default();
    p.defaults.insert(PolicyAction::MarkRead, Effect::Block);
    server.save_policy(p.clone()).await.unwrap();
    assert!(
        server
            .call_tool("read_message", &json!({"messageId":id,"markRead":true}))
            .await
            .is_err()
    );
    assert!(
        server
            .call_tool("read_message", &json!({"messageId":id}))
            .await
            .is_ok()
    );
    p.defaults.insert(PolicyAction::Read, Effect::Block);
    server.save_policy(p).await.unwrap();
    assert!(
        server
            .call_tool("reply_message", &json!({"messageId":id,"body":"Answer"}))
            .await
            .is_err()
    );
}
#[tokio::test]
async fn mcp_pending_list_never_exposes_native_message_body_or_paths() {
    let (server, _, dir) = fixture(AiConfig::default()).await;
    let _ = original(&server, dir.path()).await;
    let result = server
        .call_tool("list_pending_actions", &json!({}))
        .await
        .unwrap();
    assert_eq!(result["result"].as_array().unwrap().len(), 1);
    assert!(!result.to_string().contains("PRIVATE_BODY_SENTINEL"));
    assert!(!result.to_string().contains("PRIVATE_FILENAME"));
}

async fn queued_sends(core: &Core) -> i64 {
    core.db
        .read(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM pending_actions WHERE kind='send' AND state='pending'",
                [],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn reply_ask_read_does_not_expose_original_in_mcp_summary() {
    let (server, _core, dir) = fixture(AiConfig::default()).await;
    let id = original(&server, dir.path()).await;
    let mut p = Policy::default();
    p.defaults.insert(PolicyAction::Read, Effect::Ask);
    server.save_policy(p).await.unwrap();
    let pending = server
        .call_tool(
            "reply_message",
            &json!({"messageId":id,"body":"Proposed response"}),
        )
        .await
        .unwrap();
    assert_eq!(pending["status"], "pending_confirmation");
    assert!(!pending.to_string().contains("PRIVATE_BODY_SENTINEL"));
    assert!(
        server
            .pending_actions()
            .await
            .iter()
            .any(|v| v["pendingId"] == pending["pendingId"])
    );
}
struct PausedStorage {
    inner: FakeStorage,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}
#[async_trait::async_trait]
impl flectar_mail_core::cloud_links::StorageProvider for PausedStorage {
    fn provider(&self) -> flectar_mail_core::cloud_links::CloudProvider {
        self.inner.provider()
    }
    async fn upload(
        &self,
        file: &flectar_mail_core::cloud_links::CloudFile,
        progress: flectar_mail_core::cloud_links::Progress,
    ) -> flectar_mail_core::error::Result<flectar_mail_core::cloud_links::UploadedFile> {
        self.entered.notify_one();
        self.release.notified().await;
        self.inner.upload(file, progress).await
    }
    async fn share(
        &self,
        file: &flectar_mail_core::cloud_links::UploadedFile,
        expiry: Option<&str>,
    ) -> flectar_mail_core::error::Result<flectar_mail_core::cloud_links::SharedFile> {
        self.inner.share(file, expiry).await
    }
    async fn delete(&self, id: &str) -> flectar_mail_core::error::Result<()> {
        self.inner.delete(id).await
    }
}
#[tokio::test]
async fn block_added_during_upload_prevents_queueing_and_cleans_created_objects() {
    let (server, core, dir) = fixture(AiConfig::default()).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(std::sync::Mutex::new(vec![]));
    let server = server.with_storage_provider(Arc::new(PausedStorage {
        inner: FakeStorage {
            calls: calls.clone(),
            fail_share: false,
        },
        entered: entered.clone(),
        release: release.clone(),
    }));
    let file = dir.path().join("test.txt");
    tokio::fs::write(&file, "test").await.unwrap();
    let pending = server
        .call_tool("send_message", &cloud_args(&file))
        .await
        .unwrap();
    let id = pending["pendingId"].as_str().unwrap().to_owned();
    let worker = server.clone();
    let task = tokio::spawn(async move { worker.approve_in_app(&id).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    let mut p = Policy::default();
    p.defaults.insert(PolicyAction::CloudUpload, Effect::Block);
    server.save_policy(p).await.unwrap();
    release.notify_one();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert_eq!(queued_sends(&core).await, 0);
    assert!(calls.lock().unwrap().iter().any(|c| c == "delete:NEW-ONLY"));
    assert!(
        server
            .action_history()
            .await
            .unwrap()
            .iter()
            .any(|e| e.result == "blocked_during_upload")
    );
}

#[tokio::test]
async fn download_cannot_seed_policy_files_even_when_rule_allows_downloads() {
    let (server, _core, dir) = fixture(AiConfig::default()).await;
    let mut config = Policy::default();
    config
        .defaults
        .insert(PolicyAction::Download, Effect::Allow);
    server.save_policy(config).await.unwrap();
    let args =
        json!({"messageId":123,"attachmentId":1,"folder":dir.path(),"filename":"ai-policy.json"});
    assert!(
        server
            .call_tool("download_attachment", &args)
            .await
            .unwrap_err()
            .to_string()
            .contains("app data profile")
    );
    // A fresh profile has no policy file; approving a normal download still
    // must not let an email attachment initialize one with attacker rules.
    let (fresh, _, fresh_dir) = fixture(AiConfig::default()).await;
    let policy_path = fresh_dir.path().join("ai-policy.json");
    assert!(!policy_path.exists());
    let staged=fresh.call_tool("download_attachment",&json!({"messageId":123,"attachmentId":1,"folder":fresh_dir.path(),"filename":"ai-policy.json"})).await.unwrap();
    assert!(
        fresh
            .approve_in_app(staged["pendingId"].as_str().unwrap())
            .await
            .unwrap_err()
            .to_string()
            .contains("app data profile")
    );
    assert!(!policy_path.exists());
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        let alias = outside.path().join("profile-alias");
        std::os::unix::fs::symlink(dir.path(), &alias).unwrap();
        let mut args = args.clone();
        args["folder"] = json!(alias);
        assert!(
            server
                .call_tool("download_attachment", &args)
                .await
                .unwrap_err()
                .to_string()
                .contains("app data profile")
        );
    }
}

async fn triage_fixture(core: &Core) {
    core.db.write(|c|{c.execute_batch("INSERT INTO accounts (id,email,provider,auth_kind,username,incoming_host,incoming_port,smtp_host,smtp_port,created_at,mail_protocol,settings_json) VALUES (2,'second@example.invalid','generic','password','second','invalid',993,'invalid',587,0,'imap','{}');")?;let date=chrono::Local::now().timestamp_millis();for (id,account,sender,subject)in[(100,1,"client@example.invalid","Boleto de pagamento"),(101,1,"client@example.invalid","Urgente: prazo hoje"),(102,2,"client@example.invalid","Outra conta"),(103,1,"news@example.invalid","Novidades"),(104,1,"supplier@example.invalid","Documento"),(105,1,"known@example.invalid","Olá")]{c.execute("INSERT INTO messages(id,account_id,from_addr,subject,snippet,date,list_unsubscribe) VALUES(?1,?2,?3,?4,'metadata only',?5,?6)",(id,account,sender,subject,date,if id==103{"<https://example.invalid/unsubscribe>"}else{""}))?;}c.execute("INSERT INTO attachments(message_id,filename,mime_type) VALUES(104,'nfe-proc.xml','application/xml')",[])?;c.execute("INSERT INTO contacts(email,send_count,recv_count,last_interacted) VALUES('known@example.invalid',1,0,0)",[])?;Ok(())}).await.unwrap();
}

#[tokio::test]
async fn digest_triage_correction_approval_and_sender_scope() {
    use flectar_mail_core::triage::{Category, Digest};
    use mail_mcp::policy::{Action, Effect};
    let (server, core, dir) = fixture(AiConfig::default()).await;
    triage_fixture(&core).await;
    let original = server.call_tool("get_digest", &json!({})).await.unwrap();
    assert_eq!(original["policy"]["effect"], "allow");
    let digest: Digest = serde_json::from_value(original).unwrap();
    assert_eq!(digest.items.len(), 6);
    assert_eq!(
        digest
            .items
            .iter()
            .find(|i| i.message_id == 100)
            .unwrap()
            .category,
        Category::Finance
    );
    assert_eq!(
        digest
            .items
            .iter()
            .find(|i| i.message_id == 103)
            .unwrap()
            .category,
        Category::Newsletter
    );
    assert_eq!(
        digest
            .items
            .iter()
            .find(|i| i.message_id == 104)
            .unwrap()
            .category,
        Category::Invoice
    );
    assert_eq!(
        digest
            .items
            .iter()
            .find(|i| i.message_id == 105)
            .unwrap()
            .category,
        Category::Customer
    );
    let staged = server
        .call_tool(
            "set_category",
            &json!({"messageId":100,"category":"customer"}),
        )
        .await
        .unwrap();
    assert_eq!(staged["policy"]["effect"], "ask");
    let before = core
        .get_digest(
            flectar_mail_core::triage::today(),
            flectar_mail_core::triage::today(),
            Some(1),
            Some(100),
            false,
        )
        .await
        .unwrap();
    assert_eq!(before.items[0].category, Category::Finance);
    server
        .approve_in_app(staged["pendingId"].as_str().unwrap())
        .await
        .unwrap();
    let after = server
        .call_tool("get_triage", &json!({"accountId":1,"messageId":101}))
        .await
        .unwrap();
    assert_eq!(after["items"][0]["category"], "customer");
    assert_eq!(after["items"][0]["reason"], "sender.correction");
    let other = server
        .call_tool("get_triage", &json!({"messageId":102}))
        .await
        .unwrap();
    assert_eq!(other["items"][0]["category"], "other");
    let mut config = server.policy_config().await.0;
    config.defaults.insert(Action::Categorize, Effect::Block);
    server.save_policy(config).await.unwrap();
    assert!(
        server
            .call_tool(
                "set_category",
                &json!({"messageId":100,"category":"invoice"})
            )
            .await
            .is_err()
    );
    let history = std::fs::read_to_string(dir.path().join("ai-history.jsonl")).unwrap();
    assert!(history.contains("categorize"));
    assert!(!history.contains("Boleto"));
    assert!(!history.contains("client@example.invalid"));
    assert!(!history.contains("metadata only"));
    assert!(!history.contains("nfe-proc"));
}

#[tokio::test]
async fn digest_persistence_safe_defaults_schedule_and_boundaries() {
    use flectar_mail_core::triage::{self, Settings};
    let (_, core, _dir) = fixture(AiConfig::default()).await;
    let defaults = core.digest_settings().await.unwrap();
    assert_eq!(defaults.time, "08:00");
    assert!(!defaults.email_to_self);
    assert!(!defaults.ai_enhancement);
    let settings = Settings {
        time: "09:30".into(),
        email_to_self: true,
        account_id: Some(1),
        ..Default::default()
    };
    core.save_digest_settings(settings).await.unwrap();
    assert_eq!(core.digest_settings().await.unwrap().time, "09:30");
    assert!(core.claim_daily_digest("2026-11-06".into()).await.unwrap());
    assert!(!core.claim_daily_digest("2026-11-06".into()).await.unwrap());
    assert!(core.claim_daily_digest("2026-11-07".into()).await.unwrap());
    core.db
        .write(|c| {
            c.execute("UPDATE ai_digest_settings SET value='bad json'", [])?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(!core.digest_settings().await.unwrap().email_to_self);
    let day = chrono::NaiveDate::from_ymd_opt(2026, 11, 6).unwrap();
    use chrono::TimeZone;
    let at = chrono::Local
        .from_local_datetime(&day.and_hms_opt(8, 0, 0).unwrap())
        .unwrap();
    assert!(triage::due(at, &defaults));
    assert!(!triage::due(at - chrono::Duration::seconds(1), &defaults));
    assert!(
        core.get_digest("2026-11-01".into(), "2026-12-31".into(), None, None, false)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn digest_account_policy_cannot_be_narrowed_by_ignored_fields() {
    use mail_mcp::policy::{Action, Conditions, Effect, Rule};
    let (server, core, _dir) = fixture(AiConfig::default()).await;
    triage_fixture(&core).await;
    let mut config = server.policy_config().await.0;
    config.rules.push(Rule {
        id: "block.second".into(),
        action: Action::Read,
        effect: Effect::Block,
        conditions: Conditions {
            accounts: vec![2],
            ..Default::default()
        },
        override_new_recipients: false,
    });
    server.save_policy(config).await.unwrap();
    assert!(
        server
            .call_tool("get_digest", &json!({"messageId":100}))
            .await
            .is_err()
    );
    assert!(
        server
            .call_tool("get_digest", &json!({"accountId":1}))
            .await
            .is_ok()
    );
    assert!(
        server
            .call_tool("get_triage", &json!({"messageId":102}))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn daily_self_email_stays_pending_without_network() {
    let (server, core, _dir) = fixture(AiConfig::default()).await;
    let v=server.call_tool_from_app("send_message",&json!({"accountId":1,"to":["me@example.invalid"],"subject":"Synthetic digest","body":"Synthetic counts only"})).await.unwrap();
    assert_eq!(v["status"], "pending_confirmation");
    assert_eq!(server.pending_actions().await.len(), 1);
    let draft = core
        .get_draft(v["draftId"].as_i64().unwrap())
        .await
        .unwrap();
    assert_eq!(draft.to.len(), 1);
    assert_eq!(draft.to[0].email, "me@example.invalid");
    server
        .cancel_in_app(v["pendingId"].as_str().unwrap())
        .await
        .unwrap();
}

#[tokio::test]
async fn digest_caps_results_and_excludes_outgoing_and_other_dates() {
    let (server, core, _dir) = fixture(AiConfig::default()).await;
    core.db.write(|c|{let date=chrono::Local::now().timestamp_millis();let tx=c.transaction()?;for id in 1..=5002{tx.execute("INSERT INTO messages(id,account_id,from_addr,subject,date) VALUES(?1,1,'synthetic@example.invalid','Local metadata',?2)",(id,date))?;}tx.execute("INSERT INTO messages(id,account_id,date,is_outgoing) VALUES(6000,1,?1,1)",[date])?;tx.execute("INSERT INTO messages(id,account_id,date,is_draft) VALUES(6001,1,?1,1)",[date])?;tx.execute("INSERT INTO messages(id,account_id,date) VALUES(6002,1,0)",[])?;tx.commit()?;Ok(())}).await.unwrap();
    let v = server.call_tool("get_digest", &json!({})).await.unwrap();
    assert_eq!(v["truncated"], true);
    assert_eq!(v["items"].as_array().unwrap().len(), 5000);
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["message_id"].as_i64().unwrap() <= 5002)
    );
}
