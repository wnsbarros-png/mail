use super::*;
use policy::{Action, Config, Context, Decision, Effect, Event};

#[derive(Debug)]
pub(super) struct RoutedError {
    pub decision: Decision,
    pub details: String,
}
impl std::fmt::Display for RoutedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (policy: {})", self.details, self.decision.reason)
    }
}
impl std::error::Error for RoutedError {}
const RESULT_TTL: Duration = Duration::from_secs(10 * 60);
pub(super) struct Completed {
    action: Action,
    context: Context,
    tool: Option<(String, Value)>,
    result: Value,
    created: Instant,
    size: usize,
}
impl Server {
    pub async fn policy_config(&self) -> (Config, bool) {
        self.policy_store.load()
    }
    pub async fn save_policy(&self, config: Config) -> Result<Config> {
        config.validate()?;
        let previous = self.policy_config().await.0;
        let intent = Event::new(
            "desktop_settings",
            Action::Control,
            &Context::default(),
            &previous.evaluate(Action::Control, &Context::default(), false),
            "policy_change_requested",
            None,
        );
        let guard = self.policy_io.lock().await;
        let store = self.policy_store.clone();
        let config = tokio::task::spawn_blocking(move || {
            store.append(&intent)?;
            store.save(config)
        })
        .await??;
        drop(guard);
        let decision = config.evaluate(Action::Control, &Context::default(), false);
        self.record(
            "desktop_settings",
            Action::Control,
            &Context::default(),
            &decision,
            "policy_updated",
            None,
        )
        .await
        .map_err(|e| {
            anyhow!("Policy was saved, but final audit write failed. Reload before retrying: {e}")
        })?;
        Ok(config)
    }
    pub async fn action_history(&self) -> Result<Vec<Event>> {
        let _guard = self.policy_io.lock().await;
        let store = self.policy_store.clone();
        tokio::task::spawn_blocking(move || store.history()).await?
    }
    async fn record(
        &self,
        actor: &str,
        action: Action,
        ctx: &Context,
        d: &Decision,
        result: &str,
        id: Option<&str>,
    ) -> Result<()> {
        let event = Event::new(actor, action, ctx, d, result, id);
        let _guard = self.policy_io.lock().await;
        let store = self.policy_store.clone();
        tokio::task::spawn_blocking(move || store.append(&event)).await??;
        Ok(())
    }
    async fn decision(&self, a: Action, c: &Context) -> Decision {
        let (config, fallback) = self.policy_config().await;
        config.evaluate(a, c, fallback)
    }
    async fn recipients_context(&self, mut ctx: Context) -> Result<Context> {
        ctx.recipients = ctx
            .recipients
            .into_iter()
            .map(|v| v.to_ascii_lowercase())
            .collect();
        ctx.recipients.sort();
        ctx.recipients.dedup();
        ctx.accounts.sort_unstable();
        ctx.accounts.dedup();
        if ctx.recipients.len() > 1000 {
            bail!("AI policy supports at most 1000 unique recipients");
        }
        let recipients = ctx.recipients.clone();
        let (known,new)=self.core.db.read(move|conn|{
   let mut known=0;let mut new=0;
   for email in recipients {
    let saved:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM contact_values WHERE kind='EMAIL' AND value=?1 COLLATE NOCASE)",[&email],|r|r.get(0))?;
    let sent:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM contacts WHERE email=?1 COLLATE NOCASE AND send_count>0)",[&email],|r|r.get(0))?;
    known+=usize::from(saved);new+=usize::from(!saved&&!sent);
   } Ok((known,new))
  }).await?;
        ctx.known_count = known;
        ctx.new_count = new;
        Ok(ctx)
    }
    async fn draft_context(&self, draft: &SaveDraftArgs, cloud: bool) -> Result<Context> {
        self.recipients_context(Context {
            accounts: vec![draft.account_id],
            recipients: draft
                .to
                .iter()
                .chain(&draft.cc)
                .chain(&draft.bcc)
                .map(|a| a.email.clone())
                .collect(),
            has_attachments: !draft.attachments.is_empty() || cloud,
            has_cloud_links: cloud,
            recipient_metadata: true,
            attachment_metadata: true,
            ..Default::default()
        })
        .await
    }
    async fn tool_context(&self, name: &str, args: &Value) -> Result<Context> {
        let mut ctx = Context::default();
        let mut account = if matches!(
            name,
            "send_message"
                | "list_folders"
                | "list_messages"
                | "search_messages"
                | "get_new_messages"
                | "sync_now"
                | "get_digest"
                | "get_triage"
        ) {
            arg_i64(args, "accountId")
        } else {
            None
        };
        ctx.marks_read = name == "read_message" && arg_bool(args, "markRead").unwrap_or(false);
        if matches!(name, "send_message" | "reply_message") {
            ctx.recipient_metadata = true;
            ctx.attachment_metadata = true;
            if name == "send_message" {
                ctx.recipients = parse_addresses(args.get("to"))?
                    .into_iter()
                    .chain(parse_addresses(args.get("cc"))?)
                    .chain(parse_addresses(args.get("bcc"))?)
                    .map(|a| a.email)
                    .collect();
            } else {
                let id = req_i64(args, "messageId")?;
                let original = self
                    .core
                    .db
                    .read(move |c| flectar_mail_core::db::repo::messages::detail(c, id))
                    .await?;
                account = Some(original.account_id);
                let own = self
                    .core
                    .list_accounts()
                    .await?
                    .into_iter()
                    .filter(|a| a.id == original.account_id)
                    .map(|a| a.email)
                    .collect::<Vec<_>>();
                let (to, cc) =
                    reply_recipients(&original, &own, arg_bool(args, "replyAll").unwrap_or(false));
                ctx.recipients = to
                    .into_iter()
                    .chain(cc)
                    .chain(parse_addresses(args.get("extraCc"))?)
                    .map(|a| a.email)
                    .collect();
            }
            let (files, cloud) = self
                .prepare_attachments(args, account.ok_or_else(|| anyhow!("missing accountId"))?)
                .await?;
            ctx.has_attachments = !files.is_empty();
            ctx.has_cloud_links = cloud.is_some();
        } else {
            // Never let ignored/extra JSON fields narrow the trusted operation scope.
            let mut ids = if matches!(name, "mark_read" | "delete_message") {
                args.get("messageIds")
                    .and_then(Value::as_array)
                    .map(|v| v.iter().filter_map(Value::as_i64).collect::<Vec<_>>())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if matches!(
                name,
                "read_message" | "download_attachment" | "get_triage" | "set_category"
            ) && let Some(id) = arg_i64(args, "messageId")
            {
                ids.push(id);
            }
            let mut complete = !ids.is_empty();
            for id in ids {
                let detail = self
                    .core
                    .db
                    .read(
                        move |c| match flectar_mail_core::db::repo::messages::detail(c, id) {
                            Ok(d) => Ok(Some(d)),
                            Err(flectar_mail_core::error::CoreError::NotFound(_)) => Ok(None),
                            Err(e) => Err(e),
                        },
                    )
                    .await?;
                if let Some(d) = detail {
                    ctx.accounts.push(d.account_id);
                    ctx.has_attachments |= !d.attachments.is_empty();
                    ctx.recipients
                        .extend(d.to.into_iter().chain(d.cc).map(|a| a.email));
                } else {
                    complete = false;
                }
            }
            ctx.recipient_metadata = complete;
            ctx.attachment_metadata = complete;
        }
        if let Some(id) = account {
            ctx.accounts.push(id);
        } else if ctx.accounts.is_empty()
            && !matches!(
                name,
                "confirm_action" | "cancel_action" | "list_pending_actions" | "get_action_result"
            )
        {
            ctx.accounts = self
                .core
                .list_accounts()
                .await?
                .into_iter()
                .map(|a| a.id)
                .collect();
        }
        self.recipients_context(ctx).await
    }
    fn with_policy(value: Value, d: &Decision) -> Value {
        let mut v = wrap_structured(value);
        v["policy"] = json!(d);
        v
    }
    async fn finish_record(
        &self,
        actor: &str,
        a: Action,
        ctx: &Context,
        d: &Decision,
        result: Result<Value>,
    ) -> Result<Value> {
        let status = match &result {
            Ok(v) => v
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("succeeded"),
            Err(_) => "failed",
        };
        let id = result.as_ref().ok().and_then(|v| v["pendingId"].as_str());
        let audit = self.record(actor, a, ctx, d, status, id).await;
        match result {
            Ok(v) => {
                let mut v = Self::with_policy(v, d);
                if audit.is_err() {
                    v["auditIncomplete"] = json!(true);
                    v["warning"] = json!(
                        "Action may already have executed. Final audit write failed; do not retry automatically."
                    );
                }
                Ok(v)
            }
            Err(e) if e.is::<policy::Denied>() || e.is::<RoutedError>() => Err(e),
            Err(e) => Err(RoutedError {
                decision: d.clone(),
                details: format!("{e:#}"),
            }
            .into()),
        }
    }
    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<Value> {
        self.call_tool_as(name, args, "mcp_client").await
    }
    /// Native UI/scheduler entry; all policy and mandatory audit checks still apply.
    pub async fn call_tool_from_app(&self, name: &str, args: &Value) -> Result<Value> {
        self.call_tool_as(name, args, "desktop_digest").await
    }
    async fn call_tool_as(&self, name: &str, args: &Value, actor: &str) -> Result<Value> {
        let action = if let Some(action) = Action::for_tool(name, args) {
            action
        } else {
            let context = Context::default();
            let mut decision = self.decision(Action::Control, &context).await;
            decision.effect = Effect::Block;
            decision.reason = "unknown_tool".into();
            decision.applied[0].effect = Effect::Block;
            decision.applied[0].rule_id = "invariant.unknown_tool".into();
            self.record(actor, Action::Control, &context, &decision, "blocked", None)
                .await?;
            return Err(policy::Denied(decision).into());
        };
        let ctx = match self.tool_context(name, args).await {
            Ok(ctx) => ctx,
            Err(e) => {
                let ctx = Context::default();
                let d = self.decision(action, &ctx).await;
                self.record(actor, action, &ctx, &d, "invalid_input", None)
                    .await?;
                return Err(RoutedError {
                    decision: d,
                    details: format!("{e:#}"),
                }
                .into());
            }
        };
        let decision = self.decision(action, &ctx).await;
        self.record(actor, action, &ctx, &decision, "started", None)
            .await
            .map_err(|_| RoutedError {
                decision: decision.clone(),
                details: "Action not executed: mandatory audit history could not be written."
                    .into(),
            })?;
        if decision.effect == Effect::Block {
            self.record(actor, action, &ctx, &decision, "blocked", None)
                .await?;
            return Err(policy::Denied(decision).into());
        }
        if self.pending_actions().await.len() >= 100 && decision.effect == Effect::Ask {
            bail!("Too many pending actions; approve or cancel existing proposals first");
        }
        let result = if decision.effect == Effect::Ask
            && !matches!(
                action,
                Action::Send | Action::Reply | Action::Delete | Action::PermanentDelete
            ) {
            let id = new_pending_id();
            let summary = json!({"action":"tool_request","tool":name,"accountIds":ctx.accounts,"request":args,"policy":decision});
            self.pending.lock().await.insert(
                id.clone(),
                Pending {
                    kind: PendingKind::Tool {
                        name: name.into(),
                        args: args.clone(),
                    },
                    policy: decision.clone(),
                    context: ctx.clone(),
                    summary: summary.clone(),
                    created: Instant::now(),
                },
            );
            Ok(json!({"status":"pending_confirmation","pendingId":id,"summary":summary}))
        } else {
            self.dispatch(name, args, &decision, &ctx).await
        };
        let result=result.map(|mut v|{if action==Action::Reply && decision.applied.iter().any(|r|r.action==Action::Read && r.effect==Effect::Ask) && v["status"]=="pending_confirmation" {v["summary"]=json!({"action":"reply","policy":decision,"note":"Review the original message and proposed reply in the desktop app."});}v});
        self.finish_record(actor, action, &ctx, &decision, result)
            .await
    }
    pub async fn stage_cloud_draft(
        &self,
        draft: SaveDraftArgs,
        options: LinkOptions,
    ) -> Result<Value> {
        let action = if draft.mode.starts_with("reply") {
            Action::Reply
        } else {
            Action::Send
        };
        let ctx = self.draft_context(&draft, true).await?;
        let decision = self.decision(action, &ctx).await;
        self.record("desktop_compose", action, &ctx, &decision, "started", None)
            .await?;
        if decision.effect == Effect::Block {
            self.record("desktop_compose", action, &ctx, &decision, "blocked", None)
                .await?;
            return Err(policy::Denied(decision).into());
        }
        let result = self
            .stage_cloud_checked(draft, options, &decision, &ctx)
            .await;
        self.finish_record("desktop_compose", action, &ctx, &decision, result)
            .await
    }
    pub(super) async fn expire_pending(&self) {
        let expired = {
            let mut pending = self.pending.lock().await;
            let ids = pending
                .iter()
                .filter(|(_, p)| p.created.elapsed() >= PENDING_TTL)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| pending.remove(&id).map(|p| (id, p)))
                .collect::<Vec<_>>()
        };
        for (id, p) in expired {
            let action = p.policy.applied[0].action;
            let _ = self
                .record(
                    "system",
                    action,
                    &p.context,
                    &p.policy,
                    "expired",
                    Some(&id),
                )
                .await;
        }
    }
    async fn pending_decision(&self, p: &Pending) -> Result<(Context, Decision)> {
        let action = p.policy.applied[0].action;
        let ctx = match &p.kind {
            PendingKind::Send { draft_id, .. } => {
                self.draft_context(&self.core.get_draft(*draft_id).await?, false)
                    .await?
            }
            PendingKind::Cloud { draft_id, .. } => {
                self.draft_context(&self.core.get_draft(*draft_id).await?, true)
                    .await?
            }
            PendingKind::Delete {
                message_ids,
                permanent,
            } => {
                self.tool_context(
                    "delete_message",
                    &json!({"messageIds":message_ids,"permanent":permanent}),
                )
                .await?
            }
            PendingKind::Tool { name, args } => self.tool_context(name, args).await?,
        };
        let decision = self.decision(action, &ctx).await;
        Ok((ctx, decision))
    }
    pub(super) async fn confirm(&self, id: &str, in_app: bool) -> Result<Value> {
        let p = self.take_pending(id).await?;
        let action = p.policy.applied[0].action;
        let (ctx, d) = match self.pending_decision(&p).await {
            Ok(v) => v,
            Err(e) => {
                self.record(
                    if in_app {
                        "desktop_approval"
                    } else {
                        "mcp_confirmation"
                    },
                    action,
                    &p.context,
                    &p.policy,
                    "invalid_at_approval",
                    Some(id),
                )
                .await?;
                return Err(e);
            }
        };
        let actor = if in_app {
            "desktop_approval"
        } else {
            "mcp_confirmation"
        };
        if d.effect == Effect::Block {
            self.record(actor, action, &ctx, &d, "blocked_at_approval", Some(id))
                .await?;
            return Err(policy::Denied(d).into());
        }
        if d.in_app_only && !in_app {
            bail!("Cloud upload approval must happen in the desktop app");
        }
        self.record(actor, action, &ctx, &d, "approved", Some(id))
            .await?;
        let tool = match &p.kind {
            PendingKind::Tool { name, args } => Some((name.clone(), args.clone())),
            _ => None,
        };
        let result = self.execute_pending(p, in_app).await.map(|v| {
            let mut v = wrap_structured(v);
            v["pendingId"] = json!(id);
            v
        });
        let mut result = self.finish_record(actor, action, &ctx, &d, result).await?;
        let size = serde_json::to_vec(&result)?.len()
            + tool
                .as_ref()
                .map(|(_, args)| args.to_string().len())
                .unwrap_or_default();
        if size <= 16 * 1024 * 1024 {
            let mut completed = self.completed.lock().await;
            completed.retain(|_, c| c.created.elapsed() < RESULT_TTL);
            while !completed.is_empty()
                && (completed.len() >= 32
                    || completed.values().map(|c| c.size).sum::<usize>() + size > 16 * 1024 * 1024)
            {
                let oldest = completed
                    .iter()
                    .min_by_key(|(_, c)| c.created)
                    .map(|(id, _)| id.clone())
                    .unwrap();
                completed.remove(&oldest);
            }
            completed.insert(
                id.into(),
                Completed {
                    action,
                    context: ctx,
                    tool,
                    result: result.clone(),
                    created: Instant::now(),
                    size,
                },
            );
            result["resultAvailable"] = json!(true);
        } else {
            result["resultAvailable"] = json!(false);
            result["warning"] = json!(
                "Approved output exceeds the 16 MiB in-memory result cache; it cannot be retrieved. Do not retry a mutation."
            );
        }
        Ok(result)
    }
    pub(super) async fn get_action_result(&self, id: &str) -> Result<Value> {
        let (action, context, tool, result) = {
            let mut cache = self.completed.lock().await;
            cache.retain(|_, c| c.created.elapsed() < RESULT_TTL);
            let Some(c) = cache.get(id) else {
                return Ok(
                    json!({"status":"not_available","pendingId":id,"note":"Still pending, expired, evicted, failed, cancelled, or from a previous process; do not retry a mutation automatically."}),
                );
            };
            (
                c.action,
                c.context.clone(),
                c.tool.clone(),
                c.result.clone(),
            )
        };
        let ctx = if let Some((name, args)) = tool {
            self.tool_context(&name, &args).await?
        } else {
            self.recipients_context(context).await?
        };
        let decision = self.decision(action, &ctx).await;
        if decision.effect == Effect::Block {
            self.record(
                "mcp_client",
                action,
                &ctx,
                &decision,
                "result_access_blocked",
                Some(id),
            )
            .await?;
            return Err(policy::Denied(decision).into());
        }
        self.record(
            "mcp_client",
            action,
            &ctx,
            &decision,
            "approved_result_retrieved",
            Some(id),
        )
        .await?;
        Ok(json!({"status":"completed","pendingId":id,"accessPolicy":decision,"result":result}))
    }
    pub(super) async fn recheck_cloud(&self, p: &Pending) -> Result<()> {
        let (ctx, d) = self.pending_decision(p).await?;
        if d.effect == Effect::Block {
            self.record(
                "system",
                p.policy.applied[0].action,
                &ctx,
                &d,
                "blocked_during_upload",
                None,
            )
            .await?;
            return Err(policy::Denied(d).into());
        }
        Ok(())
    }
    pub(super) async fn record_cancel(&self, p: &Pending, id: &str, actor: &str) -> Result<()> {
        self.record(
            actor,
            p.policy.applied[0].action,
            &p.context,
            &p.policy,
            "cancelled",
            Some(id),
        )
        .await
    }
}
