use flectar_mail_core::Core;
use flectar_mail_core::accounts::credentials::UnavailableCredentialStore;
use flectar_mail_core::config::Paths;
use flectar_mail_core::models::{Address, MessageDetail, SenderVerification};
use mail_mcp::*;
use serde_json::{Value, json};
use std::sync::Arc;

async fn server(config: AiConfig) -> (Server, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::start_mail_ui_with_credentials(
        Paths::for_tests(dir.path()),
        Arc::new(UnavailableCredentialStore),
    )
    .await
    .unwrap();
    (Server::new(core, config), dir)
}

async fn call(s: &Server, id: i64, method: &str, params: Value) -> Value {
    s.handle(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
        .await
        .unwrap()
}

fn tool_result(v: &Value) -> (&Value, bool) {
    (
        &v["result"]["structuredContent"],
        v["result"]["isError"].as_bool().unwrap(),
    )
}

#[tokio::test]
async fn initialize_and_list_tools() {
    let (s, _d) = server(AiConfig::default()).await;
    let init = call(
        &s,
        1,
        "initialize",
        json!({"protocolVersion": PROTOCOL_VERSION}),
    )
    .await;
    assert_eq!(init["result"]["serverInfo"]["name"], branding::SERVER_NAME);
    assert!(
        s.handle(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await
            .is_none()
    );
    let tools = call(&s, 2, "tools/list", json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for want in [
        "list_accounts",
        "list_messages",
        "search_messages",
        "read_message",
        "send_message",
        "reply_message",
        "delete_message",
        "mark_read",
        "get_new_messages",
        "confirm_action",
        "cancel_action",
    ] {
        assert!(names.contains(&want), "missing {want}");
    }
    let send = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "send_message")
        .unwrap();
    assert!(
        send["description"]
            .as_str()
            .unwrap()
            .contains("per-action AI policy")
    );
    let bad = call(&s, 3, "nope", json!({})).await;
    assert_eq!(bad["error"]["code"], -32601);
}

#[tokio::test]
async fn empty_profile_reads() {
    let (s, _d) = server(AiConfig::default()).await;
    let r = call(
        &s,
        1,
        "tools/call",
        json!({"name":"list_accounts","arguments":{}}),
    )
    .await;
    let (v, err) = tool_result(&r);
    assert!(!err);
    assert_eq!(v["result"], json!([]));
    let r = call(
        &s,
        2,
        "tools/call",
        json!({"name":"list_messages","arguments":{"unreadOnly":true}}),
    )
    .await;
    assert_eq!(tool_result(&r).0["messages"], json!([]));
    let r = call(
        &s,
        3,
        "tools/call",
        json!({"name":"search_messages","arguments":{"query":"invoice"}}),
    )
    .await;
    assert!(!tool_result(&r).1);
    let r = call(
        &s,
        4,
        "tools/call",
        json!({"name":"get_new_messages","arguments":{}}),
    )
    .await;
    assert_eq!(tool_result(&r).0["cursor"], 0);
    let r = call(
        &s,
        5,
        "tools/call",
        json!({"name":"get_new_messages","arguments":{"cursor":0,"waitSeconds":1}}),
    )
    .await;
    assert_eq!(tool_result(&r).0["messages"], json!([]));
}

#[tokio::test]
async fn unsafe_actions_are_guarded() {
    let (s, _d) = server(AiConfig::default()).await;
    // Delete is staged, not executed.
    let r = call(
        &s,
        1,
        "tools/call",
        json!({"name":"delete_message","arguments":{"messageIds":[42]}}),
    )
    .await;
    let (v, err) = tool_result(&r);
    assert!(!err);
    assert_eq!(v["status"], "pending_confirmation");
    let pid = v["pendingId"].as_str().unwrap().to_string();
    let r = call(
        &s,
        2,
        "tools/call",
        json!({"name":"list_pending_actions","arguments":{}}),
    )
    .await;
    assert_eq!(tool_result(&r).0["result"].as_array().unwrap().len(), 1);
    let r = call(
        &s,
        3,
        "tools/call",
        json!({"name":"cancel_action","arguments":{"pendingId":pid}}),
    )
    .await;
    assert_eq!(tool_result(&r).0["status"], "cancelled");
    let r = call(
        &s,
        4,
        "tools/call",
        json!({"name":"confirm_action","arguments":{"pendingId":pid}}),
    )
    .await;
    assert!(tool_result(&r).1, "a cancelled action cannot be confirmed");
    // Permanent delete is off by default.
    let r = call(
        &s,
        5,
        "tools/call",
        json!({"name":"delete_message","arguments":{"messageIds":[1],"permanent":true}}),
    )
    .await;
    assert!(tool_result(&r).1);
    // Sending needs a real account and recipients.
    let r = call(
        &s,
        6,
        "tools/call",
        json!({"name":"send_message","arguments":{"accountId":1,"to":[],"subject":"x","body":"y"}}),
    )
    .await;
    assert!(tool_result(&r).1);
    let r = call(&s, 7, "tools/call", json!({"name":"send_message","arguments":{"accountId":1,"to":["not-an-address"],"subject":"x","body":"y"}})).await;
    assert!(tool_result(&r).1);
}

#[test]
fn config_defaults_are_safe_and_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let c = AiConfig::load_or_init(dir.path());
    assert!(c.ai_actions_require_confirmation);
    assert!(!c.allow_permanent_delete);
    assert!(!c.allow_mcp_confirmation);
    assert!(dir.path().join(branding::CONFIG_FILE).exists());
    std::fs::write(dir.path().join(branding::CONFIG_FILE), "{not json").unwrap();
    assert!(AiConfig::load_or_init(dir.path()).ai_actions_require_confirmation);
    std::fs::write(
        dir.path().join(branding::CONFIG_FILE),
        r#"{"ai_actions_require_confirmation":false}"#,
    )
    .unwrap();
    assert!(!AiConfig::load_or_init(dir.path()).ai_actions_require_confirmation);
}

#[test]
fn address_parsing() {
    let a = parse_addresses(Some(&json!([
        "Ana Silva <ana@example.com>",
        "bob@example.com"
    ])))
    .unwrap();
    assert_eq!(a[0].name.as_deref(), Some("Ana Silva"));
    assert_eq!(a[0].email, "ana@example.com");
    assert_eq!(a[1].email, "bob@example.com");
    assert_eq!(
        parse_addresses(Some(&json!("a@example.com, b@example.com")))
            .unwrap()
            .len(),
        2
    );
    assert!(parse_addresses(Some(&json!(["nope"]))).is_err());
}

fn detail(from: &str, to: &[&str], cc: &[&str], outgoing: bool) -> MessageDetail {
    let addr = |e: &&str| Address {
        name: None,
        email: e.to_string(),
    };
    MessageDetail {
        id: 1,
        thread_id: 1,
        account_id: 1,
        from: addr(&from),
        to: to.iter().map(addr).collect(),
        cc: cc.iter().map(addr).collect(),
        subject: "Hi".into(),
        date: 1_700_000_000_000,
        is_read: true,
        is_starred: false,
        is_draft: false,
        is_outgoing: outgoing,
        snippet: String::new(),
        body_state: "ready".into(),
        text_body: None,
        html_body: None,
        automation_note: None,
        attachments: vec![],
        list_unsubscribe: None,
        list_unsubscribe_post: None,
        via: None,
        sender_verification: SenderVerification::default(),
        send_state: None,
        send_error: None,
    }
}

#[test]
fn reply_recipient_rules() {
    let own = vec!["me@example.com".to_string()];
    let m = detail(
        "ana@example.com",
        &["me@example.com", "bob@example.com"],
        &["carl@example.com"],
        false,
    );
    let (to, cc) = reply_recipients(&m, &own, false);
    assert_eq!(
        to.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(),
        ["ana@example.com"]
    );
    assert!(cc.is_empty());
    let (to, cc) = reply_recipients(&m, &own, true);
    assert_eq!(to.len(), 1);
    assert_eq!(
        cc.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(),
        ["bob@example.com", "carl@example.com"]
    );
    let sent = detail("me@example.com", &["ana@example.com"], &[], true);
    let (to, _) = reply_recipients(&sent, &own, false);
    assert_eq!(to[0].email, "ana@example.com");
}

#[test]
fn text_helpers() {
    assert_eq!(reply_subject("Hello"), "Re: Hello");
    assert_eq!(reply_subject("RE: Hello"), "RE: Hello");
    let q = quote_body(
        &Address {
            name: Some("Ana".into()),
            email: "ana@example.com".into(),
        },
        1_700_000_000_000,
        "line1\n\nline2",
    );
    assert!(q.contains("Ana <ana@example.com> wrote:"));
    assert!(q.contains("> line1\n>\n> line2"));
    assert_eq!(
        html_to_text("<style>x{}</style><p>Hi &amp; bye</p><br>ok"),
        "Hi & bye\n\nok"
    );
}
