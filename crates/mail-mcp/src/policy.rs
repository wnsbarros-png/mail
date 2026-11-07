//! One policy boundary for all AI tools. No arguments/bodies enter the audit log.
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    List,
    Read,
    Search,
    MarkRead,
    Send,
    Reply,
    Delete,
    PermanentDelete,
    CloudUpload,
    Download,
    Sync,
    Categorize,
    Followup,
    Control,
}
pub const ACTIONS: [Action; 13] = [
    Action::List,
    Action::Read,
    Action::Search,
    Action::MarkRead,
    Action::Send,
    Action::Reply,
    Action::Delete,
    Action::PermanentDelete,
    Action::CloudUpload,
    Action::Download,
    Action::Sync,
    Action::Categorize,
    Action::Followup,
];
impl Action {
    pub fn for_tool(tool: &str, args: &serde_json::Value) -> Option<Self> {
        Some(match tool {
            "list_accounts" | "list_folders" | "list_messages" | "list_templates" => Self::List,
            "read_message" | "get_new_messages" | "get_digest" | "get_triage"
            | "render_template" | "list_followups" => Self::Read,
            "search_messages" => Self::Search,
            "mark_read" => Self::MarkRead,
            "set_category" => Self::Categorize,
            "set_followup" | "cancel_followup" => Self::Followup,
            "send_message" => Self::Send,
            "reply_message" => Self::Reply,
            "delete_message" => {
                if args["permanent"].as_bool() == Some(true) {
                    Self::PermanentDelete
                } else {
                    Self::Delete
                }
            }
            "download_attachment" => Self::Download,
            "sync_now" => Self::Sync,
            "confirm_action" | "cancel_action" | "list_pending_actions" | "get_action_result" => {
                Self::Control
            }
            _ => return None,
        })
    }
    pub fn default_effect(self) -> Effect {
        match self {
            Self::List | Self::Read | Self::Search | Self::Control => Effect::Allow,
            Self::PermanentDelete => Effect::Block,
            _ => Effect::Ask,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Allow,
    Ask,
    Block,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Conditions {
    pub accounts: Vec<i64>,
    pub known_contacts: Option<bool>,
    pub addresses: Vec<String>,
    pub domains: Vec<String>,
    pub new_recipients: Option<bool>,
    pub min_recipients: Option<usize>,
    pub max_recipients: Option<usize>,
    pub has_attachments: Option<bool>,
    pub has_cloud_links: Option<bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub action: Action,
    pub effect: Effect,
    #[serde(default)]
    pub conditions: Conditions,
    #[serde(default)]
    pub override_new_recipients: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub defaults: BTreeMap<Action, Effect>,
    #[serde(default)]
    pub rules: Vec<Rule>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            revision: 0,
            defaults: ACTIONS
                .into_iter()
                .map(|a| (a, a.default_effect()))
                .collect(),
            rules: vec![],
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub uses_template: bool,
    pub followup: bool,
    pub accounts: Vec<i64>,
    pub recipients: Vec<String>,
    pub known_count: usize,
    pub new_count: usize,
    pub has_attachments: bool,
    pub has_cloud_links: bool,
    pub marks_read: bool,
    pub recipient_metadata: bool,
    pub attachment_metadata: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Applied {
    pub action: Action,
    pub rule_id: String,
    pub effect: Effect,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Decision {
    pub effect: Effect,
    pub applied: Vec<Applied>,
    pub reason: String,
    pub in_app_only: bool,
    pub revision: u64,
    pub safe_fallback: bool,
}
impl Conditions {
    fn matches(&self, c: &Context, effect: Effect) -> bool {
        let restrictive = effect != Effect::Allow;
        let account = self.accounts.is_empty()
            || (!c.accounts.is_empty()
                && if restrictive {
                    c.accounts.iter().any(|a| self.accounts.contains(a))
                } else {
                    c.accounts.iter().all(|a| self.accounts.contains(a))
                });
        let allowlist = self.addresses.is_empty() && self.domains.is_empty()
            || (!c.recipients.is_empty() && {
                let allowed = |a: &String| {
                    self.addresses.iter().any(|v| v.eq_ignore_ascii_case(a))
                        || a.rsplit_once('@').is_some_and(|(_, d)| {
                            self.domains.iter().any(|v| v.eq_ignore_ascii_case(d))
                        })
                };
                if restrictive {
                    c.recipients.iter().any(allowed)
                } else {
                    c.recipients.iter().all(allowed)
                }
            });
        let recipients_match = allowlist
            && self.known_contacts.is_none_or(|v| {
                v == (!c.recipients.is_empty() && c.known_count == c.recipients.len())
            })
            && self.new_recipients.is_none_or(|v| v == (c.new_count > 0))
            && self.min_recipients.is_none_or(|v| c.recipients.len() >= v)
            && self.max_recipients.is_none_or(|v| c.recipients.len() <= v);
        let has_recipient_condition = !self.addresses.is_empty()
            || !self.domains.is_empty()
            || self.known_contacts.is_some()
            || self.new_recipients.is_some()
            || self.min_recipients.is_some()
            || self.max_recipients.is_some();
        let recipients_match = if !c.recipient_metadata && has_recipient_condition {
            restrictive
        } else {
            recipients_match
        };
        let has_attachment_condition =
            self.has_attachments.is_some() || self.has_cloud_links.is_some();
        let attachments_match = if !c.attachment_metadata && has_attachment_condition {
            restrictive
        } else {
            self.has_attachments.is_none_or(|v| v == c.has_attachments)
                && self.has_cloud_links.is_none_or(|v| v == c.has_cloud_links)
        };
        account && recipients_match && attachments_match
    }
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.rules.len() > 128
            || self.defaults.contains_key(&Action::Control)
        {
            bail!("Unsupported or invalid policy version/configuration");
        }
        let mut ids = std::collections::HashSet::new();
        for r in &self.rules {
            if r.id.is_empty()
                || r.id.len() > 64
                || !r
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
                || !ids.insert(&r.id)
                || r.action == Action::Control
            {
                bail!(
                    "Rule IDs must be unique, 1–64 ASCII letters/digits/_/./-; control rules are reserved"
                );
            }
            let c = &r.conditions;
            if c.accounts.len() > 100
                || c.accounts.iter().any(|id| *id <= 0)
                || c.addresses.len() + c.domains.len() > 1000
                || c.min_recipients
                    .zip(c.max_recipients)
                    .is_some_and(|(a, b)| a > b)
                || c.max_recipients.is_some_and(|n| n > 1000)
                || c.min_recipients.is_some_and(|n| n > 1000)
            {
                bail!("Invalid account/count conditions");
            }
            for d in &c.domains {
                if !valid_domain(d) {
                    bail!("Use exact ASCII domains without @, wildcard or URL");
                }
            }
            for a in &c.addresses {
                if !a.is_ascii()
                    || a.chars().any(char::is_whitespace)
                    || a.chars().any(char::is_control)
                    || a.matches('@').count() != 1
                    || !a
                        .split_once('@')
                        .is_some_and(|(l, d)| !l.is_empty() && valid_domain(d))
                {
                    bail!("Use exact email addresses, not display names or wildcards");
                }
            }
            if r.override_new_recipients
                && (r.effect != Effect::Allow || !matches!(r.action, Action::Send | Action::Reply))
            {
                bail!("New-recipient override only applies to explicit Allow send/reply rules");
            }
        }
        Ok(())
    }
    fn facet(&self, a: Action, c: &Context) -> (Applied, bool) {
        let matching = self
            .rules
            .iter()
            .filter(|r| r.action == a && r.conditions.matches(c, r.effect))
            .collect::<Vec<_>>();
        let r = matching
            .iter()
            .find(|r| r.effect == Effect::Block)
            .copied()
            .or_else(|| matching.first().copied());
        match r {
            Some(r) => (
                Applied {
                    action: a,
                    rule_id: r.id.clone(),
                    effect: r.effect,
                },
                r.override_new_recipients,
            ),
            None => (
                Applied {
                    action: a,
                    rule_id: format!("default.{a:?}").to_ascii_lowercase(),
                    effect: self
                        .defaults
                        .get(&a)
                        .copied()
                        .unwrap_or_else(|| a.default_effect()),
                },
                false,
            ),
        }
    }
    /// The ONLY evaluator. Mandatory invariants are applied after configurable rules.
    pub fn evaluate(&self, a: Action, c: &Context, safe_fallback: bool) -> Decision {
        if a == Action::Control {
            return Decision {
                effect: Effect::Allow,
                applied: vec![Applied {
                    action: a,
                    rule_id: "invariant.review_controls".into(),
                    effect: Effect::Allow,
                }],
                reason: "review_controls_only; underlying action is rechecked".into(),
                in_app_only: false,
                revision: self.revision,
                safe_fallback,
            };
        }
        let (primary, override_new) = self.facet(a, c);
        let mut applied = vec![primary];
        if a == Action::Reply || (a == Action::Send && c.uses_template) {
            applied.push(self.facet(Action::Read, c).0);
        }
        if a == Action::Read && c.marks_read {
            applied.push(self.facet(Action::MarkRead, c).0);
        }
        if c.has_cloud_links && matches!(a, Action::Send | Action::Reply) {
            applied.push(self.facet(Action::CloudUpload, c).0);
        }
        let mut effect = if applied.iter().any(|r| r.effect == Effect::Block) {
            Effect::Block
        } else if applied.iter().any(|r| r.effect == Effect::Ask) {
            Effect::Ask
        } else {
            Effect::Allow
        };
        let mut reason = if effect == Effect::Block {
            if a == Action::PermanentDelete {
                "permanent deletion is disabled by policy"
            } else {
                "blocked_by_rule"
            }
        } else {
            "matched_rule_or_safe_default"
        }
        .to_owned();
        let cloud = c.has_cloud_links || a == Action::CloudUpload;
        if effect != Effect::Block && cloud {
            effect = Effect::Ask;
            reason = "invariant.cloud_requires_app_approval".into();
        } else if effect != Effect::Block && c.followup {
            effect = Effect::Ask;
            reason = "invariant.followup_requires_app_approval".into();
        } else if effect == Effect::Allow
            && matches!(a, Action::Send | Action::Reply)
            && c.new_count > 0
            && !override_new
        {
            effect = Effect::Ask;
            reason = "invariant.new_recipient_requires_approval".into();
        }
        Decision {
            effect,
            applied,
            reason,
            in_app_only: cloud || c.followup,
            revision: self.revision,
            safe_fallback,
        }
    }
}
fn valid_domain(d: &str) -> bool {
    !d.is_empty()
        && d.len() <= 253
        && d.is_ascii()
        && d.split('.').all(|p| {
            !p.is_empty()
                && p.len() <= 63
                && !p.starts_with('-')
                && !p.ends_with('-')
                && p.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}
#[derive(Debug)]
pub struct Denied(pub Decision);
impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "AI policy blocked action: {} (rules: {})",
            self.0.reason,
            self.0
                .applied
                .iter()
                .map(|r| r.rule_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}
impl std::error::Error for Denied {}
#[derive(Clone)]
pub struct Store {
    root: PathBuf,
}
impl Store {
    pub fn new(root: &Path) -> Self {
        Self { root: root.into() }
    }
    pub fn load(&self) -> (Config, bool) {
        let path = self.root.join("ai-policy.json");
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            return (Config::default(), false);
        };
        if !meta.is_file() || meta.len() > 256 * 1024 {
            return (Config::default(), true);
        }
        let parsed = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Config>(&b).ok())
            .filter(|c| c.validate().is_ok());
        parsed.map_or_else(|| (Config::default(), true), |c| (c, false))
    }
    pub fn save(&self, mut config: Config) -> Result<Config> {
        config.validate()?;
        let (previous, _) = self.load();
        config.revision = previous.revision.saturating_add(1);
        let mut tmp = tempfile::NamedTempFile::new_in(&self.root)?;
        tmp.write_all(&serde_json::to_vec_pretty(&config)?)?;
        tmp.as_file().sync_all()?;
        tmp.persist(self.root.join("ai-policy.json"))?;
        Ok(config)
    }
    pub fn append(&self, event: &Event) -> Result<()> {
        let path = self.root.join("ai-history.jsonl");
        if std::fs::symlink_metadata(&path).is_ok_and(|m| !m.is_file()) {
            bail!("History path must be a regular file");
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true).append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(path)?;
        if file.metadata()?.len() > 0 {
            file.seek(SeekFrom::End(-1))?;
            let mut last = [0];
            file.read_exact(&mut last)?;
            if last[0] != b'\n' {
                file.write_all(b"\n")?;
            }
        }
        let mut line = serde_json::to_vec(event)?;
        line.push(b'\n');
        file.write_all(&line)?;
        file.sync_data()?;
        Ok(())
    }
    pub fn history(&self) -> Result<Vec<Event>> {
        let path = self.root.join("ai-history.jsonl");
        if !path.exists() {
            return Ok(vec![]);
        }
        if !std::fs::symlink_metadata(&path)?.is_file() {
            bail!("History path must be a regular file");
        }
        let mut file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        let start = len.saturating_sub(256 * 1024);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.take(256 * 1024).read_to_end(&mut bytes)?;
        let text = String::from_utf8_lossy(&bytes);
        let mut events = text
            .lines()
            .skip(usize::from(start > 0))
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect::<Vec<Event>>();
        if events.len() > 200 {
            events.drain(..events.len() - 200);
        }
        Ok(events)
    }
}
/// Strict schema: no raw arguments, addresses, subjects, bodies, paths or errors.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub version: u32,
    pub id: String,
    pub timestamp_ms: i64,
    pub actor: String,
    pub action: Action,
    pub accounts: Vec<i64>,
    pub recipient_count: usize,
    pub new_recipient_count: usize,
    pub has_attachments: bool,
    pub has_cloud_links: bool,
    pub decision: Decision,
    pub result: String,
    pub pending_id: Option<String>,
}
impl Event {
    pub fn new(
        actor: &str,
        action: Action,
        c: &Context,
        d: &Decision,
        result: &str,
        pending_id: Option<&str>,
    ) -> Self {
        Self {
            version: 1,
            id: format!("{:032x}", rand::random::<u128>()),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            actor: actor.into(),
            action,
            accounts: c.accounts.clone(),
            recipient_count: c.recipients.len(),
            new_recipient_count: c.new_count,
            has_attachments: c.has_attachments,
            has_cloud_links: c.has_cloud_links,
            decision: d.clone(),
            result: result.into(),
            pending_id: pending_id
                .filter(|id| {
                    id.strip_prefix("p-").is_some_and(|hex| {
                        hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit())
                    })
                })
                .map(String::from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ctx() -> Context {
        Context {
            accounts: vec![1],
            recipients: vec!["alice@example.invalid".into()],
            known_count: 1,
            new_count: 0,
            has_attachments: false,
            has_cloud_links: false,
            marks_read: false,
            recipient_metadata: true,
            attachment_metadata: true,
        }
    }
    fn rule(effect: Effect, conditions: Conditions) -> Rule {
        Rule {
            id: "contacts".into(),
            action: Action::Send,
            effect,
            conditions,
            override_new_recipients: false,
        }
    }
    #[test]
    fn defaults_matrix() {
        let c = Config::default();
        for a in ACTIONS {
            assert_eq!(c.evaluate(a, &ctx(), false).effect, a.default_effect());
        }
        assert_eq!(
            c.evaluate(Action::Control, &ctx(), false).effect,
            Effect::Allow
        );
    }
    #[test]
    fn each_condition_matrix() {
        let cases = [
            (
                Conditions {
                    accounts: vec![1],
                    ..Default::default()
                },
                true,
            ),
            (
                Conditions {
                    accounts: vec![2],
                    ..Default::default()
                },
                false,
            ),
            (
                Conditions {
                    known_contacts: Some(true),
                    ..Default::default()
                },
                true,
            ),
            (
                Conditions {
                    known_contacts: Some(false),
                    ..Default::default()
                },
                false,
            ),
            (
                Conditions {
                    addresses: vec!["ALICE@example.invalid".into()],
                    ..Default::default()
                },
                true,
            ),
            (
                Conditions {
                    addresses: vec!["bob@example.invalid".into()],
                    ..Default::default()
                },
                false,
            ),
            (
                Conditions {
                    domains: vec!["example.invalid".into()],
                    ..Default::default()
                },
                true,
            ),
            (
                Conditions {
                    domains: vec!["invalid".into()],
                    ..Default::default()
                },
                false,
            ),
            (
                Conditions {
                    new_recipients: Some(false),
                    ..Default::default()
                },
                true,
            ),
            (
                Conditions {
                    new_recipients: Some(true),
                    ..Default::default()
                },
                false,
            ),
            (
                Conditions {
                    min_recipients: Some(1),
                    max_recipients: Some(1),
                    ..Default::default()
                },
                true,
            ),
            (
                Conditions {
                    max_recipients: Some(0),
                    ..Default::default()
                },
                false,
            ),
            (
                Conditions {
                    has_attachments: Some(false),
                    ..Default::default()
                },
                true,
            ),
            (
                Conditions {
                    has_attachments: Some(true),
                    ..Default::default()
                },
                false,
            ),
            (
                Conditions {
                    has_cloud_links: Some(false),
                    ..Default::default()
                },
                true,
            ),
            (
                Conditions {
                    has_cloud_links: Some(true),
                    ..Default::default()
                },
                false,
            ),
        ];
        for (conditions, expected) in cases {
            let cfg = Config {
                rules: vec![rule(Effect::Allow, conditions)],
                ..Default::default()
            };
            assert_eq!(
                cfg.evaluate(Action::Send, &ctx(), false).effect == Effect::Allow,
                expected
            );
        }
    }
    #[test]
    fn conjunction_and_mixed_recipient_protection() {
        let mut context = ctx();
        context.recipients.push("bob@other.invalid".into());
        context.accounts.push(2);
        context.known_count = 2;
        for effect in [Effect::Allow, Effect::Ask, Effect::Block] {
            let c = Conditions {
                accounts: vec![1],
                domains: vec!["example.invalid".into()],
                ..Default::default()
            };
            assert_eq!(c.matches(&context, effect), effect != Effect::Allow);
        }
        let cfg = Config {
            rules: vec![rule(
                Effect::Allow,
                Conditions {
                    known_contacts: Some(true),
                    has_attachments: Some(true),
                    ..Default::default()
                },
            )],
            ..Default::default()
        };
        assert_eq!(
            cfg.evaluate(Action::Send, &context, false).effect,
            Effect::Ask
        );
    }
    #[test]
    fn blocks_win_and_nonblock_order_is_stable() {
        let mut cfg = Config::default();
        let mut allow = rule(Effect::Allow, Conditions::default());
        allow.id = "allow".into();
        let mut ask = rule(Effect::Ask, Conditions::default());
        ask.id = "ask".into();
        cfg.rules = vec![allow.clone(), ask.clone()];
        assert_eq!(
            cfg.evaluate(Action::Send, &ctx(), false).effect,
            Effect::Allow
        );
        cfg.rules.reverse();
        assert_eq!(
            cfg.evaluate(Action::Send, &ctx(), false).effect,
            Effect::Ask
        );
        let mut block = rule(Effect::Block, Conditions::default());
        block.id = "block".into();
        cfg.rules.push(block);
        assert_eq!(
            cfg.evaluate(Action::Send, &ctx(), false).effect,
            Effect::Block
        );
        assert_eq!(
            cfg.evaluate(Action::Send, &ctx(), false).applied[0].rule_id,
            "block"
        );
    }
    #[test]
    fn new_recipients_require_explicit_rule_override() {
        let mut ctx = ctx();
        ctx.known_count = 0;
        ctx.new_count = 1;
        let mut cfg = Config::default();
        cfg.defaults.insert(Action::Send, Effect::Allow);
        assert_eq!(cfg.evaluate(Action::Send, &ctx, false).effect, Effect::Ask);
        cfg.rules.push(rule(Effect::Allow, Conditions::default()));
        assert_eq!(cfg.evaluate(Action::Send, &ctx, false).effect, Effect::Ask);
        cfg.rules[0].override_new_recipients = true;
        assert_eq!(
            cfg.evaluate(Action::Send, &ctx, false).effect,
            Effect::Allow
        );
        cfg.rules[0].effect = Effect::Block;
        assert_eq!(
            cfg.evaluate(Action::Send, &ctx, false).effect,
            Effect::Block
        );
    }
    #[test]
    fn cloud_cannot_bypass_app_approval() {
        for action in [Action::Send, Action::Reply, Action::CloudUpload] {
            let mut cfg = Config::default();
            cfg.defaults.insert(action, Effect::Allow);
            cfg.defaults.insert(Action::CloudUpload, Effect::Allow);
            let mut ctx = ctx();
            ctx.has_cloud_links = true;
            let d = cfg.evaluate(action, &ctx, false);
            assert_eq!(d.effect, Effect::Ask);
            assert!(d.in_app_only);
            cfg.defaults.insert(Action::CloudUpload, Effect::Block);
            assert_eq!(cfg.evaluate(action, &ctx, false).effect, Effect::Block);
        }
    }
    #[test]
    fn versioned_persistence_and_invalid_files_fail_safe() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let mut cfg = Config::default();
        cfg.rules.push(rule(
            Effect::Allow,
            Conditions {
                domains: vec!["example.invalid".into()],
                ..Default::default()
            },
        ));
        let saved = store.save(cfg).unwrap();
        assert_eq!(saved.revision, 1);
        assert_eq!(
            store.load().0.rules[0].conditions.domains,
            ["example.invalid"]
        );
        assert!(!store.load().1);
        for text in [
            "not json",
            r#"{"version":99}"#,
            r#"{"version":1,"surprise":true}"#,
            r#"{"version":1,"defaults":{"bogus":"allow"}}"#,
        ] {
            std::fs::write(dir.path().join("ai-policy.json"), text).unwrap();
            let (cfg, fallback) = store.load();
            assert!(fallback);
            assert_eq!(
                cfg.evaluate(Action::PermanentDelete, &ctx(), fallback)
                    .effect,
                Effect::Block
            );
            assert_eq!(
                cfg.evaluate(Action::Send, &ctx(), fallback).effect,
                Effect::Ask
            );
        }
    }
    #[test]
    fn invalid_rules_are_rejected() {
        let invalid = [
            Conditions {
                domains: vec!["*.example.invalid".into()],
                ..Default::default()
            },
            Conditions {
                addresses: vec!["Display Name <a@example.invalid>".into()],
                ..Default::default()
            },
            Conditions {
                accounts: vec![-1],
                ..Default::default()
            },
            Conditions {
                min_recipients: Some(2),
                max_recipients: Some(1),
                ..Default::default()
            },
        ];
        for c in invalid {
            assert!(
                Config {
                    rules: vec![rule(Effect::Allow, c)],
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
    }
    #[test]
    fn append_only_history_is_minimal_and_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let context = ctx();
        let d = Config::default().evaluate(Action::Send, &context, false);
        let event = Event::new(
            "mcp_client",
            Action::Send,
            &context,
            &d,
            "pending",
            Some("p-test"),
        );
        store.append(&event).unwrap();
        let first = std::fs::read(dir.path().join("ai-history.jsonl")).unwrap();
        store.append(&event).unwrap();
        assert!(
            std::fs::read(dir.path().join("ai-history.jsonl"))
                .unwrap()
                .starts_with(&first)
        );
        let text = String::from_utf8(first).unwrap();
        assert!(!text.contains("alice@example.invalid"));
        assert!(!text.contains("body"));
        assert!(!text.contains("credential"));
        assert_eq!(store.history().unwrap().len(), 2);
        for _ in 0..205 {
            store.append(&event).unwrap();
        }
        assert_eq!(store.history().unwrap().len(), 200);
    }
    #[cfg(unix)]
    #[test]
    fn storage_permissions_and_symlinks() {
        use std::os::unix::{fs::PermissionsExt, fs::symlink};
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store.save(Config::default()).unwrap();
        assert_eq!(
            std::fs::metadata(dir.path().join("ai-policy.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let event = Event::new(
            "mcp_client",
            Action::Read,
            &ctx(),
            &Config::default().evaluate(Action::Read, &ctx(), false),
            "succeeded",
            None,
        );
        store.append(&event).unwrap();
        assert_eq!(
            std::fs::metadata(dir.path().join("ai-history.jsonl"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        std::fs::remove_file(dir.path().join("ai-history.jsonl")).unwrap();
        symlink(
            dir.path().join("ai-policy.json"),
            dir.path().join("ai-history.jsonl"),
        )
        .unwrap();
        assert!(store.append(&event).is_err());
        assert!(store.history().is_err());
    }
    #[test]
    fn partial_history_is_preserved_and_new_events_recover() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        std::fs::write(dir.path().join("ai-history.jsonl"), b"{partial-crash").unwrap();
        let context = ctx();
        let decision = Config::default().evaluate(Action::Read, &context, false);
        store
            .append(&Event::new(
                "mcp_client",
                Action::Read,
                &context,
                &decision,
                "succeeded",
                None,
            ))
            .unwrap();
        let bytes = std::fs::read(dir.path().join("ai-history.jsonl")).unwrap();
        assert!(bytes.starts_with(b"{partial-crash\n"));
        assert_eq!(store.history().unwrap().len(), 1);
    }
    #[test]
    fn oversized_and_symlink_policy_files_fail_safe() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let path = dir.path().join("ai-policy.json");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(256 * 1024 + 1)
            .unwrap();
        assert!(store.load().1);
        #[cfg(unix)]
        {
            std::fs::remove_file(&path).unwrap();
            let target = dir.path().join("other.json");
            std::fs::write(&target, r#"{"version":1,"defaults":{"send":"allow"}}"#).unwrap();
            std::os::unix::fs::symlink(target, &path).unwrap();
            assert!(store.load().1);
        }
    }
    #[test]
    fn arbitrary_client_values_cannot_become_audit_pending_ids() {
        let context = ctx();
        let decision = Config::default().evaluate(Action::Control, &context, false);
        let event = Event::new(
            "mcp_client",
            Action::Control,
            &context,
            &decision,
            "not_available",
            Some("PRIVATE_BODY_SENTINEL"),
        );
        assert!(event.pending_id.is_none());
        assert!(
            !serde_json::to_string(&event)
                .unwrap()
                .contains("PRIVATE_BODY_SENTINEL")
        );
    }
    #[test]
    fn missing_metadata_never_matches_conditional_allow_and_restrictive_rules_fail_closed() {
        let context = Context {
            accounts: vec![1],
            ..Default::default()
        };
        for conditions in [
            Conditions {
                domains: vec!["example.invalid".into()],
                ..Default::default()
            },
            Conditions {
                has_attachments: Some(true),
                ..Default::default()
            },
            Conditions {
                known_contacts: Some(true),
                max_recipients: Some(3),
                ..Default::default()
            },
        ] {
            assert!(!conditions.matches(&context, Effect::Allow));
            assert!(conditions.matches(&context, Effect::Block));
            assert!(conditions.matches(&context, Effect::Ask));
        }
    }
}
