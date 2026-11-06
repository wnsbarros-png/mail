//! Local Model Context Protocol server for the mail client.
//!
//! The server speaks JSON-RPC 2.0 over stdio (one JSON message per line), so
//! it is reachable only by the process that launched it: no network socket is
//! opened. It drives the same [`Core`] the desktop application embeds, using
//! the same database and the same OS-keyring credentials, so every protocol
//! (Gmail/Outlook OAuth, IMAP/SMTP, JMAP, POP3) works without new code.
//!
//! Sending and deleting are two-step by default: the first call only stages a
//! *pending action* (a real draft for sends, visible in the app's Drafts) and
//! returns its id and a summary; nothing leaves the machine until
//! `confirm_action` is called with that id. See `docs/ai-mcp.md`.

pub mod branding;

use anyhow::{Context, Result, anyhow, bail};
use flectar_mail_core::Core;
use flectar_mail_core::MessagePageRequest;
use flectar_mail_core::models::{
    ActionKind, ActionTarget, Address, MessageDetail, MessageSummary, PerformActionArgs,
    QueueSendArgs, SaveDraftArgs, View,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub const PROTOCOL_VERSION: &str = "2025-06-18";
const MAX_LIMIT: i64 = 100;
const PENDING_TTL: Duration = Duration::from_secs(30 * 60);
const BODY_WAIT: Duration = Duration::from_secs(15);
const MAX_BODY_CHARS: usize = 100_000;

/// User-editable settings, stored as JSON in the data directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AiConfig {
    /// Send, reply and delete only stage a pending action until confirmed.
    pub ai_actions_require_confirmation: bool,
    /// Whether `delete_message` may bypass Trash.
    pub allow_permanent_delete: bool,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            ai_actions_require_confirmation: true,
            allow_permanent_delete: false,
        }
    }
}

impl AiConfig {
    /// Load the config, writing the safe defaults on first run so the user can
    /// find and edit the file. An unreadable file falls back to the defaults
    /// (confirmation on), never to a more permissive setting.
    pub fn load_or_init(data_dir: &Path) -> Self {
        let path = data_dir.join(branding::CONFIG_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
                tracing::warn!("ignoring invalid {}: {error}", path.display());
                Self::default()
            }),
            Err(_) => {
                let config = Self::default();
                if let Ok(text) = serde_json::to_string_pretty(&config) {
                    let _ = std::fs::write(&path, text);
                }
                config
            }
        }
    }
}

#[derive(Debug, Clone)]
enum PendingKind {
    Send {
        draft_id: i64,
    },
    Delete {
        message_ids: Vec<i64>,
        permanent: bool,
    },
}

#[derive(Debug, Clone)]
struct Pending {
    kind: PendingKind,
    summary: Value,
    created: Instant,
}

/// The tool layer. Cheap to clone; all state is shared.
#[derive(Clone)]
pub struct Server {
    core: Core,
    config: AiConfig,
    pending: Arc<Mutex<HashMap<String, Pending>>>,
}

fn arg_i64(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(Value::as_i64)
}
fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}
fn arg_bool(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(Value::as_bool)
}
fn req_i64(args: &Value, key: &str) -> Result<i64> {
    arg_i64(args, key).ok_or_else(|| anyhow!("missing integer argument `{key}`"))
}
fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    arg_str(args, key).ok_or_else(|| anyhow!("missing string argument `{key}`"))
}
fn limit(args: &Value, default: i64) -> i64 {
    arg_i64(args, "limit")
        .unwrap_or(default)
        .clamp(1, MAX_LIMIT)
}

/// Parse `"Name <a@b>"`, `"a@b"`, or `{"name":..,"email":..}` entries.
pub fn parse_addresses(value: Option<&Value>) -> Result<Vec<Address>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items: Vec<Value> = match value {
        Value::Null => return Ok(Vec::new()),
        Value::Array(items) => items.clone(),
        Value::String(s) => s
            .split([',', ';'])
            .map(|p| Value::String(p.to_string()))
            .collect(),
        other => vec![other.clone()],
    };
    let mut out = Vec::new();
    for item in items {
        let address = match item {
            Value::String(s) => {
                let s = s.trim();
                if s.is_empty() {
                    continue;
                }
                match (s.rfind('<'), s.rfind('>')) {
                    (Some(start), Some(end)) if start < end => {
                        let name = s[..start].trim().trim_matches('"').trim();
                        Address {
                            name: (!name.is_empty()).then(|| name.to_string()),
                            email: s[start + 1..end].trim().to_string(),
                        }
                    }
                    _ => Address {
                        name: None,
                        email: s.to_string(),
                    },
                }
            }
            Value::Object(_) => serde_json::from_value(item).context("invalid address object")?,
            _ => bail!("addresses must be strings or {{name,email}} objects"),
        };
        if !address.email.contains('@') || address.email.contains(char::is_whitespace) {
            bail!("invalid email address `{}`", address.email);
        }
        out.push(address);
    }
    Ok(out)
}

fn format_address(a: &Address) -> String {
    match &a.name {
        Some(name) => format!("{name} <{}>", a.email),
        None => a.email.clone(),
    }
}

/// Timestamps in the store are epoch milliseconds or seconds; render RFC 3339.
pub fn iso_time(ts: i64) -> String {
    let ms = if ts.abs() < 100_000_000_000 {
        ts * 1000
    } else {
        ts
    };
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

pub fn reply_subject(subject: &str) -> String {
    let lower = subject.trim_start().to_lowercase();
    if lower.starts_with("re:") {
        subject.to_string()
    } else {
        format!("Re: {subject}")
    }
}

/// The original text quoted below the reply, mail-client style.
pub fn quote_body(from: &Address, date: i64, text: &str) -> String {
    let mut out = format!(
        "\n\nOn {}, {} wrote:\n",
        iso_time(date),
        format_address(from)
    );
    for line in text.lines() {
        out.push('>');
        if !line.is_empty() {
            out.push(' ');
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Recipients for a reply: the sender (or, for our own message, its original
/// recipients); reply-all adds every other participant except our addresses.
pub fn reply_recipients(
    original: &MessageDetail,
    own: &[String],
    all: bool,
) -> (Vec<Address>, Vec<Address>) {
    let is_own = |a: &Address| own.iter().any(|o| o.eq_ignore_ascii_case(&a.email));
    let mut to: Vec<Address> = if original.is_outgoing || is_own(&original.from) {
        original.to.clone()
    } else {
        vec![original.from.clone()]
    };
    let mut cc = Vec::new();
    if all {
        for a in original.to.iter().chain(original.cc.iter()) {
            if !is_own(a)
                && !to
                    .iter()
                    .chain(cc.iter())
                    .any(|b: &Address| b.email.eq_ignore_ascii_case(&a.email))
            {
                cc.push(a.clone());
            }
        }
    }
    to.retain(|a| !is_own(a) || original.is_outgoing);
    (to, cc)
}

fn summary_json(m: &MessageSummary) -> Value {
    json!({
        "messageId": m.id,
        "threadId": m.thread_id,
        "accountId": m.account_id,
        "account": m.account_email,
        "from": format_address(&m.from),
        "to": m.to.iter().map(format_address).collect::<Vec<_>>(),
        "subject": m.subject,
        "snippet": m.snippet,
        "date": iso_time(m.date),
        "isRead": m.is_read,
        "isStarred": m.is_starred,
        "hasAttachments": m.has_attachments,
    })
}

fn parse_view(name: Option<&str>) -> Result<View> {
    Ok(
        match name.unwrap_or("inbox").to_ascii_lowercase().as_str() {
            "inbox" => View::Inbox,
            "starred" => View::Starred,
            "snoozed" => View::Snoozed,
            "sent" => View::Sent,
            "drafts" => View::Drafts,
            "archive" | "done" => View::Done,
            "trash" => View::Trash,
            "spam" | "junk" => View::Spam,
            "all" => View::All,
            other => bail!(
                "unknown folder `{other}`; use inbox, sent, drafts, archive, trash, spam, starred, all, or pass folderId"
            ),
        },
    )
}

fn new_pending_id() -> String {
    use rand::Rng;
    let n: u64 = rand::rng().random();
    format!("p-{n:016x}")
}

impl Server {
    pub fn new(core: Core, config: AiConfig) -> Self {
        Self {
            core,
            config,
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn config(&self) -> &AiConfig {
        &self.config
    }

    /// JSON schemas advertised by `tools/list`.
    pub fn tool_definitions(&self) -> Value {
        let confirm_note = if self.config.ai_actions_require_confirmation {
            " Requires confirmation: this only stages a pending action; show the summary to the user and call confirm_action after they approve."
        } else {
            ""
        };
        let addr = json!({"type":"array","items":{"type":"string"},"description":"Addresses like \"Name <a@b.com>\" or \"a@b.com\""});
        json!([
          {"name":"list_accounts","description":"List configured mail accounts (Gmail, Outlook, IMAP/SMTP such as GoDaddy or Hostinger, JMAP, POP3) with their sync state.","inputSchema":{"type":"object","properties":{}}},
          {"name":"list_folders","description":"List folders/mailboxes of one account or all accounts.","inputSchema":{"type":"object","properties":{"accountId":{"type":"integer"}}}},
          {"name":"list_messages","description":"List recent messages, newest first, from a standard folder or a folder id.","inputSchema":{"type":"object","properties":{
              "accountId":{"type":"integer","description":"Omit for all accounts"},
              "folder":{"type":"string","enum":["inbox","sent","drafts","archive","trash","spam","starred","snoozed","all"]},
              "folderId":{"type":"integer","description":"Specific folder from list_folders (overrides folder)"},
              "unreadOnly":{"type":"boolean"},
              "limit":{"type":"integer","minimum":1,"maximum":100}}}},
          {"name":"search_messages","description":"Full-text search over locally synced mail. Supports the app's search syntax, e.g. from:alice subject:invoice is:unread has:attachment.","inputSchema":{"type":"object","required":["query"],"properties":{
              "query":{"type":"string"},"accountId":{"type":"integer"},"limit":{"type":"integer","minimum":1,"maximum":100}}}},
          {"name":"read_message","description":"Read one message: headers, plain-text body and attachment metadata. Downloads the body from the server if needed.","inputSchema":{"type":"object","required":["messageId"],"properties":{
              "messageId":{"type":"integer"},"markRead":{"type":"boolean","description":"Also mark it read (default false)"}}}},
          {"name":"send_message",
           "description":format!("Compose and send a new email from an account.{confirm_note}"),
           "inputSchema":{"type":"object","required":["accountId","to","subject","body"],"properties":{
              "accountId":{"type":"integer"},"to":addr,"cc":addr,"bcc":addr,
              "subject":{"type":"string"},"body":{"type":"string","description":"Plain text body"}}}},
          {"name":"reply_message",
           "description":format!("Reply (or reply-all) to a message, quoting the original.{confirm_note}"),
           "inputSchema":{"type":"object","required":["messageId","body"],"properties":{
              "messageId":{"type":"integer"},"body":{"type":"string"},
              "replyAll":{"type":"boolean"},"quoteOriginal":{"type":"boolean","description":"Default true"},
              "extraCc":addr}}},
          {"name":"delete_message",
           "description":format!("Move messages to Trash (or delete permanently if allowed in settings).{confirm_note}"),
           "inputSchema":{"type":"object","required":["messageIds"],"properties":{
              "messageIds":{"type":"array","items":{"type":"integer"}},"permanent":{"type":"boolean"}}}},
          {"name":"mark_read","description":"Mark messages read or unread.","inputSchema":{"type":"object","required":["messageIds"],"properties":{
              "messageIds":{"type":"array","items":{"type":"integer"}},"read":{"type":"boolean","description":"Default true"}}}},
          {"name":"get_new_messages","description":"Monitoring: return inbox messages that arrived after `cursor` (pass the cursor from the previous call; omit it the first time to get the current cursor plus the latest messages). Optionally waits up to waitSeconds for new mail.","inputSchema":{"type":"object","properties":{
              "cursor":{"type":"integer"},"accountId":{"type":"integer"},
              "waitSeconds":{"type":"integer","minimum":0,"maximum":120},"limit":{"type":"integer","minimum":1,"maximum":100}}}},
          {"name":"sync_now","description":"Ask the mail engine to check the server for new mail now.","inputSchema":{"type":"object","properties":{"accountId":{"type":"integer"}}}},
          {"name":"list_pending_actions","description":"List staged send/reply/delete actions waiting for confirmation.","inputSchema":{"type":"object","properties":{}}},
          {"name":"confirm_action","description":"Execute a staged send/reply/delete. Only call this after the user explicitly approved that exact action.","inputSchema":{"type":"object","required":["pendingId"],"properties":{"pendingId":{"type":"string"}}}},
          {"name":"cancel_action","description":"Discard a staged action (a staged email's draft is deleted).","inputSchema":{"type":"object","required":["pendingId"],"properties":{"pendingId":{"type":"string"}}}}
        ])
    }

    /// Run one tool. Errors become `isError` results for the client.
    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<Value> {
        match name {
            "list_accounts" => self.list_accounts().await,
            "list_folders" => {
                let folders = self.core.list_folders(arg_i64(args, "accountId")).await?;
                Ok(json!(
                    folders
                        .iter()
                        .map(|f| json!({
                            "folderId": f.id, "accountId": f.account_id, "name": f.display_name,
                            "path": f.path, "role": f.role, "selectable": f.selectable
                        }))
                        .collect::<Vec<_>>()
                ))
            }
            "list_messages" => self.list_messages(args).await,
            "search_messages" => self.search(args).await,
            "read_message" => self.read_message(args).await,
            "send_message" => self.send_message(args).await,
            "reply_message" => self.reply_message(args).await,
            "delete_message" => self.delete_message(args).await,
            "mark_read" => {
                let ids = message_ids(args)?;
                let kind = if arg_bool(args, "read").unwrap_or(true) {
                    ActionKind::MarkRead
                } else {
                    ActionKind::MarkUnread
                };
                self.core
                    .perform_action(PerformActionArgs {
                        kind,
                        target: ActionTarget::Messages(ids.clone()),
                        params: None,
                    })
                    .await?;
                Ok(json!({"updated": ids}))
            }
            "get_new_messages" => self.new_messages(args).await,
            "sync_now" => {
                self.core.sync_now(arg_i64(args, "accountId")).await?;
                Ok(json!({"requested": true}))
            }
            "list_pending_actions" => {
                let mut pending = self.pending.lock().await;
                pending.retain(|_, p| p.created.elapsed() < PENDING_TTL);
                Ok(json!(
                    pending
                        .iter()
                        .map(|(id, p)| json!({"pendingId": id, "summary": p.summary}))
                        .collect::<Vec<_>>()
                ))
            }
            "confirm_action" => self.confirm(req_str(args, "pendingId")?).await,
            "cancel_action" => self.cancel(req_str(args, "pendingId")?).await,
            other => bail!("unknown tool `{other}`"),
        }
    }

    async fn list_accounts(&self) -> Result<Value> {
        let accounts = self.core.list_accounts().await?;
        Ok(json!(accounts.iter().map(|a| json!({
            "accountId": a.id, "email": a.email, "name": a.display_name,
            "provider": format!("{:?}", a.provider), "protocol": format!("{:?}", a.mail_protocol),
            "syncState": a.sync_state, "syncError": a.sync_error,
        })).collect::<Vec<_>>()))
    }

    async fn list_messages(&self, args: &Value) -> Result<Value> {
        let view = parse_view(arg_str(args, "folder"))?;
        let unread_only = arg_bool(args, "unreadOnly").unwrap_or(false);
        let want = limit(args, 20);
        let folder_id = arg_i64(args, "folderId");
        let view = if folder_id.is_some() { View::All } else { view };
        let mut out = Vec::new();
        let mut cursor = None;
        // Page until enough rows survive the unread filter (bounded).
        for _ in 0..10 {
            let page = self
                .core
                .list_messages(
                    view,
                    None,
                    arg_i64(args, "accountId"),
                    None,
                    folder_id,
                    MessagePageRequest {
                        cursor,
                        limit: want.max(25),
                    },
                )
                .await?;
            out.extend(
                page.messages
                    .iter()
                    .filter(|m| !unread_only || !m.is_read)
                    .map(summary_json),
            );
            cursor = page.next_cursor;
            if out.len() as i64 >= want || cursor.is_none() {
                break;
            }
        }
        out.truncate(want as usize);
        Ok(json!({"messages": out}))
    }

    async fn search(&self, args: &Value) -> Result<Value> {
        let page = self
            .core
            .search_messages_page(
                req_str(args, "query")?.to_string(),
                arg_i64(args, "accountId"),
                None,
                limit(args, 20),
            )
            .await?;
        Ok(json!({"messages": page.messages.iter().map(summary_json).collect::<Vec<_>>()}))
    }

    /// The body, downloading it first when only headers are synced.
    async fn body(&self, message_id: i64) -> Result<MessageDetail> {
        let deadline = Instant::now() + BODY_WAIT;
        loop {
            let detail = self.core.get_body(message_id).await?;
            if detail.text_body.is_some() || detail.html_body.is_some() || Instant::now() > deadline
            {
                return Ok(detail);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn read_message(&self, args: &Value) -> Result<Value> {
        let id = req_i64(args, "messageId")?;
        let d = self.body(id).await?;
        let mut text = d
            .text_body
            .clone()
            .unwrap_or_else(|| d.html_body.as_deref().map(html_to_text).unwrap_or_default());
        let truncated = text.chars().count() > MAX_BODY_CHARS;
        if truncated {
            text = text.chars().take(MAX_BODY_CHARS).collect();
        }
        if arg_bool(args, "markRead").unwrap_or(false) && !d.is_read {
            self.core
                .perform_action(PerformActionArgs {
                    kind: ActionKind::MarkRead,
                    target: ActionTarget::Messages(vec![id]),
                    params: None,
                })
                .await?;
        }
        Ok(json!({
            "messageId": d.id, "threadId": d.thread_id, "accountId": d.account_id,
            "from": format_address(&d.from), "to": d.to.iter().map(format_address).collect::<Vec<_>>(),
            "cc": d.cc.iter().map(format_address).collect::<Vec<_>>(),
            "subject": d.subject, "date": iso_time(d.date), "isRead": d.is_read,
            "bodyState": d.body_state, "body": text, "bodyTruncated": truncated,
            "attachments": d.attachments.iter().filter(|a| !a.is_inline).map(|a| json!({
                "attachmentId": a.id, "filename": a.filename, "mimeType": a.mime_type, "size": a.size
            })).collect::<Vec<_>>(),
        }))
    }

    async fn stage_or_send(&self, draft: SaveDraftArgs, kind_label: &str) -> Result<Value> {
        let summary = json!({
            "action": kind_label,
            "accountId": draft.account_id,
            "to": draft.to.iter().map(format_address).collect::<Vec<_>>(),
            "cc": draft.cc.iter().map(format_address).collect::<Vec<_>>(),
            "bcc": draft.bcc.iter().map(format_address).collect::<Vec<_>>(),
            "subject": draft.subject,
            "body": draft.body_text,
        });
        if draft.to.is_empty() && draft.cc.is_empty() && draft.bcc.is_empty() {
            bail!("the message has no recipients");
        }
        let draft_id = self.core.save_draft(draft).await?;
        if self.config.ai_actions_require_confirmation {
            let id = new_pending_id();
            self.pending.lock().await.insert(
                id.clone(),
                Pending {
                    kind: PendingKind::Send { draft_id },
                    summary: summary.clone(),
                    created: Instant::now(),
                },
            );
            return Ok(
                json!({"status": "pending_confirmation", "pendingId": id, "draftId": draft_id, "summary": summary,
                "next": "Show this to the user. Call confirm_action only after they approve, or cancel_action to discard."}),
            );
        }
        let sent = self
            .core
            .queue_send(QueueSendArgs {
                draft_id,
                send_at: None,
            })
            .await?;
        Ok(
            json!({"status": "queued", "draftId": draft_id, "actionId": sent.action_id, "summary": summary}),
        )
    }

    async fn send_message(&self, args: &Value) -> Result<Value> {
        let draft = SaveDraftArgs {
            draft_id: None,
            account_id: req_i64(args, "accountId")?,
            from: None,
            to: parse_addresses(args.get("to"))?,
            cc: parse_addresses(args.get("cc"))?,
            bcc: parse_addresses(args.get("bcc"))?,
            subject: req_str(args, "subject")?.to_string(),
            body_text: req_str(args, "body")?.to_string(),
            body_html: None,
            mode: "new".into(),
            in_reply_to_message_id: None,
            in_reply_to_header: None,
            attachments: Vec::new(),
        };
        self.stage_or_send(draft, "send").await
    }

    async fn reply_message(&self, args: &Value) -> Result<Value> {
        let id = req_i64(args, "messageId")?;
        let all = arg_bool(args, "replyAll").unwrap_or(false);
        let original = self.body(id).await?;
        let accounts = self.core.list_accounts().await?;
        let own: Vec<String> = accounts
            .iter()
            .filter(|a| a.id == original.account_id)
            .map(|a| a.email.clone())
            .collect();
        let (to, mut cc) = reply_recipients(&original, &own, all);
        cc.extend(parse_addresses(args.get("extraCc"))?);
        let mut body = req_str(args, "body")?.to_string();
        if arg_bool(args, "quoteOriginal").unwrap_or(true) {
            let text = original
                .text_body
                .clone()
                .or_else(|| original.html_body.as_deref().map(html_to_text))
                .unwrap_or_else(|| original.snippet.clone());
            body.push_str(&quote_body(&original.from, original.date, &text));
        }
        let draft = SaveDraftArgs {
            draft_id: None,
            account_id: original.account_id,
            from: None,
            to,
            cc,
            bcc: Vec::new(),
            subject: reply_subject(&original.subject),
            body_text: body,
            body_html: None,
            mode: if all {
                "reply_all".into()
            } else {
                "reply".into()
            },
            in_reply_to_message_id: Some(id),
            in_reply_to_header: None,
            attachments: Vec::new(),
        };
        self.stage_or_send(draft, if all { "reply_all" } else { "reply" })
            .await
    }

    async fn delete_message(&self, args: &Value) -> Result<Value> {
        let ids = message_ids(args)?;
        let permanent = arg_bool(args, "permanent").unwrap_or(false);
        if permanent && !self.config.allow_permanent_delete {
            bail!(
                "permanent deletion is disabled (allow_permanent_delete=false in {}); omit `permanent` to move to Trash",
                branding::CONFIG_FILE
            );
        }
        let summary = json!({"action": if permanent {"delete_permanently"} else {"move_to_trash"}, "messageIds": ids});
        if self.config.ai_actions_require_confirmation {
            let id = new_pending_id();
            self.pending.lock().await.insert(
                id.clone(),
                Pending {
                    kind: PendingKind::Delete {
                        message_ids: ids,
                        permanent,
                    },
                    summary: summary.clone(),
                    created: Instant::now(),
                },
            );
            return Ok(
                json!({"status": "pending_confirmation", "pendingId": id, "summary": summary}),
            );
        }
        self.run_delete(ids, permanent).await
    }

    async fn run_delete(&self, ids: Vec<i64>, permanent: bool) -> Result<Value> {
        let kind = if permanent {
            ActionKind::DeletePermanently
        } else {
            ActionKind::Trash
        };
        let r = self
            .core
            .perform_action(PerformActionArgs {
                kind,
                target: ActionTarget::Messages(ids.clone()),
                params: None,
            })
            .await?;
        Ok(
            json!({"status": "done", "action": kind.as_str(), "messageIds": ids, "actionIds": r.action_ids}),
        )
    }

    async fn take_pending(&self, id: &str) -> Result<Pending> {
        let mut pending = self.pending.lock().await;
        pending.retain(|_, p| p.created.elapsed() < PENDING_TTL);
        pending.remove(id).ok_or_else(|| {
            anyhow!("no pending action `{id}` (it may have expired after 30 minutes)")
        })
    }

    async fn confirm(&self, id: &str) -> Result<Value> {
        let p = self.take_pending(id).await?;
        match p.kind {
            PendingKind::Send { draft_id } => {
                let sent = self
                    .core
                    .queue_send(QueueSendArgs {
                        draft_id,
                        send_at: None,
                    })
                    .await?;
                Ok(json!({"status": "queued", "actionId": sent.action_id, "summary": p.summary}))
            }
            PendingKind::Delete {
                message_ids,
                permanent,
            } => self.run_delete(message_ids, permanent).await,
        }
    }

    async fn cancel(&self, id: &str) -> Result<Value> {
        let p = self.take_pending(id).await?;
        if let PendingKind::Send { draft_id } = p.kind {
            self.core.delete_draft(draft_id).await?;
        }
        Ok(json!({"status": "cancelled", "summary": p.summary}))
    }

    async fn new_messages(&self, args: &Value) -> Result<Value> {
        let account = arg_i64(args, "accountId");
        let want = limit(args, 50);
        let wait =
            Duration::from_secs(arg_i64(args, "waitSeconds").unwrap_or(0).clamp(0, 120) as u64);
        let deadline = Instant::now() + wait;
        let mut events = self.core.bus.subscribe();
        loop {
            let page = self
                .core
                .list_messages(
                    View::Inbox,
                    None,
                    account,
                    None,
                    None,
                    MessagePageRequest {
                        cursor: None,
                        limit: MAX_LIMIT,
                    },
                )
                .await?;
            let max_id = page.messages.iter().map(|m| m.id).max();
            let Some(cursor) = arg_i64(args, "cursor") else {
                let latest: Vec<Value> = page
                    .messages
                    .iter()
                    .take(want as usize)
                    .map(summary_json)
                    .collect();
                return Ok(
                    json!({"cursor": max_id.unwrap_or(0), "messages": latest, "note": "Initial call: pass `cursor` next time to get only newer mail."}),
                );
            };
            let mut fresh: Vec<&MessageSummary> =
                page.messages.iter().filter(|m| m.id > cursor).collect();
            fresh.sort_by_key(|m| m.id);
            if !fresh.is_empty() || Instant::now() >= deadline {
                let next = fresh
                    .iter()
                    .map(|m| m.id)
                    .max()
                    .unwrap_or(cursor)
                    .max(cursor);
                fresh.truncate(want as usize);
                return Ok(
                    json!({"cursor": next, "messages": fresh.into_iter().map(summary_json).collect::<Vec<_>>()}),
                );
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let _ =
                tokio::time::timeout(remaining.min(Duration::from_secs(5)), events.recv()).await;
        }
    }

    /// Handle one JSON-RPC message; `None` for notifications.
    pub async fn handle(&self, msg: Value) -> Option<Value> {
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let id = id?; // notifications (no id) get no reply
        let result: std::result::Result<Value, (i64, String)> = match method {
            "initialize" => Ok(json!({
                "protocolVersion": msg.pointer("/params/protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION),
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": branding::SERVER_NAME, "title": branding::APP_NAME, "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Tools for reading and managing the user's email. Never send, reply or delete without the user's explicit request; when an action returns pending_confirmation, show the summary and wait for approval before confirm_action."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": self.tool_definitions()})),
            "tools/call" => {
                let name = msg
                    .pointer("/params/name")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let args = msg
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or(json!({}));
                Ok(match self.call_tool(name, &args).await {
                    Ok(v) => {
                        json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}], "structuredContent": wrap_structured(v), "isError": false})
                    }
                    Err(e) => {
                        json!({"content": [{"type": "text", "text": format!("Error: {e:#}")}], "isError": true})
                    }
                })
            }
            other => Err((-32601, format!("method not found: {other}"))),
        };
        Some(match result {
            Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
            Err((code, message)) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
            }
        })
    }
}

/// MCP structured content must be an object.
fn wrap_structured(v: Value) -> Value {
    if v.is_object() {
        v
    } else {
        json!({"result": v})
    }
}

fn message_ids(args: &Value) -> Result<Vec<i64>> {
    let ids: Vec<i64> = args
        .get("messageIds")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    if ids.is_empty() {
        bail!("`messageIds` must be a non-empty array of integers");
    }
    Ok(ids)
}

/// Minimal HTML-to-text for messages without a text part.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut tag = String::new();
    let mut skip = false;
    for c in html.chars() {
        if in_tag {
            if c == '>' {
                in_tag = false;
                let t = tag
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if t == "style" || t == "script" {
                    skip = !tag.starts_with('/');
                }
                if matches!(
                    t.as_str(),
                    "br" | "p" | "div" | "tr" | "li" | "h1" | "h2" | "h3"
                ) {
                    out.push('\n');
                }
                tag.clear();
            } else {
                tag.push(c);
            }
        } else if c == '<' {
            in_tag = true;
        } else if !skip {
            out.push(c);
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    let mut lines: Vec<&str> = out.lines().map(str::trim_end).collect();
    lines.dedup_by(|a, b| a.is_empty() && b.is_empty());
    lines.join("\n").trim().to_string()
}
