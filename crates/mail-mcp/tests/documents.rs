use flectar_mail_core::{Core, accounts::credentials::UnavailableCredentialStore, config::Paths};
use mail_mcp::{
    AiConfig, Server,
    policy::{Action, Conditions, Effect, Rule},
};
use serde_json::json;
use std::sync::Arc;
async fn server() -> (Server, tempfile::TempDir) {
    let d = tempfile::tempdir().unwrap();
    let c = Core::start_mail_ui_with_credentials(
        Paths::for_tests(d.path()),
        Arc::new(UnavailableCredentialStore),
    )
    .await
    .unwrap();
    (Server::new(c, AiConfig::default()), d)
}
#[tokio::test]
async fn documents_are_reads_csv_is_download_plus_read_no_body_in_history() {
    let (s, _d) = server().await;
    let r = s.call_tool("list_documents", &json!({})).await.unwrap();
    assert_eq!(r["result"], json!([]));
    assert_eq!(r["policy"]["applied"][0]["action"], "read");
    let folder = tempfile::tempdir().unwrap();
    let args = json!({"folder":folder.path().to_string_lossy()});
    let r = s.call_tool("export_documents_csv", &args).await.unwrap();
    assert_eq!(r["status"], "pending_confirmation");
    let id = r["pendingId"].as_str().unwrap();
    assert!(std::fs::read_dir(folder.path()).unwrap().next().is_none());
    s.approve_in_app(id).await.unwrap();
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    let mut cfg = s.policy_config().await.0;
    cfg.defaults.insert(Action::Read, Effect::Block);
    cfg.defaults.insert(Action::Download, Effect::Allow);
    s.save_policy(cfg).await.unwrap();
    assert!(s.call_tool("export_documents_csv", &args).await.is_err());
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    let events = serde_json::to_string(&s.action_history().await.unwrap()).unwrap();
    assert!(!events.contains("folder"));
    assert!(!events.contains(folder.path().to_string_lossy().as_ref()));
}
#[tokio::test]
async fn read_ask_and_scope_cannot_be_narrowed_by_ignored_fields() {
    let (s, _d) = server().await;
    let mut cfg = s.policy_config().await.0;
    cfg.defaults.insert(Action::Read, Effect::Ask);
    s.save_policy(cfg).await.unwrap();
    let r = s
        .call_tool("list_documents", &json!({"accountId":42}))
        .await
        .unwrap();
    assert_eq!(r["status"], "pending_confirmation");
    s.approve_in_app(r["pendingId"].as_str().unwrap())
        .await
        .unwrap();
    assert!(
        s.call_tool("list_documents", &json!({"filters":{"account_id":0}}))
            .await
            .is_err()
    );
    assert!(
        s.call_tool("extract_documents", &json!({"accountId":42}))
            .await
            .is_err()
    );
    let mut cfg = s.policy_config().await.0;
    cfg.rules.push(Rule {
        id: "never-download".into(),
        action: Action::Download,
        effect: Effect::Block,
        conditions: Conditions::default(),
        override_new_recipients: false,
    });
    s.save_policy(cfg).await.unwrap();
    assert!(
        s.call_tool("export_documents_csv", &json!({"folder":"/tmp"}))
            .await
            .is_err()
    );
}
