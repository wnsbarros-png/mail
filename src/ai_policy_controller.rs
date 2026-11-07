//! Human-only policy editor. No MCP endpoint can change policy or erase history.
use crate::{AppWindow, InboxState, PolicyRuleForm};
use mail_mcp::policy::{ACTIONS, Action, Conditions, Config, Effect, Rule};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{cell::RefCell, rc::Rc};
use tokio::runtime::Handle;
fn effect(index: i32) -> Effect {
    match index {
        0 => Effect::Allow,
        2 => Effect::Block,
        _ => Effect::Ask,
    }
}
fn effect_index(effect: Effect) -> i32 {
    match effect {
        Effect::Allow => 0,
        Effect::Ask => 1,
        Effect::Block => 2,
    }
}
fn action(index: i32) -> Result<Action, String> {
    usize::try_from(index)
        .ok()
        .and_then(|n| ACTIONS.get(n))
        .copied()
        .ok_or_else(|| "Invalid action / Ação inválida".into())
}
fn optional_bool(index: i32) -> Option<bool> {
    match index {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}
fn bool_index(v: Option<bool>) -> i32 {
    match v {
        Some(true) => 1,
        Some(false) => 2,
        None => 0,
    }
}
fn list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}
fn number(s: &str) -> Result<Option<usize>, String> {
    if s.trim().is_empty() {
        Ok(None)
    } else {
        s.trim()
            .parse()
            .map(Some)
            .map_err(|_| "Invalid recipient count / Contagem de destinatários inválida".into())
    }
}
fn form_rule(f: &PolicyRuleForm) -> Result<Rule, String> {
    let accounts = list(&f.accounts)
        .into_iter()
        .map(|s| s.parse::<i64>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Invalid account IDs / IDs de contas inválidos")?;
    Ok(Rule {
        id: f.id.trim().into(),
        action: action(f.action_index)?,
        effect: effect(f.mode_index),
        override_new_recipients: f.override_new,
        conditions: Conditions {
            accounts,
            addresses: list(&f.addresses),
            domains: list(&f.domains),
            known_contacts: optional_bool(f.known_index),
            new_recipients: optional_bool(f.new_index),
            min_recipients: number(&f.min_count)?,
            max_recipients: number(&f.max_count)?,
            has_attachments: optional_bool(f.attachments_index),
            has_cloud_links: optional_bool(f.cloud_index),
        },
    })
}
fn rule_form(r: &Rule) -> PolicyRuleForm {
    let c = &r.conditions;
    PolicyRuleForm {
        id: r.id.clone().into(),
        action_index: ACTIONS
            .iter()
            .position(|a| *a == r.action)
            .unwrap_or_default() as i32,
        mode_index: effect_index(r.effect),
        accounts: c
            .accounts
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
            .into(),
        addresses: c.addresses.join(", ").into(),
        domains: c.domains.join(", ").into(),
        known_index: bool_index(c.known_contacts),
        new_index: bool_index(c.new_recipients),
        min_count: c
            .min_recipients
            .map(|n| n.to_string())
            .unwrap_or_default()
            .into(),
        max_count: c
            .max_recipients
            .map(|n| n.to_string())
            .unwrap_or_default()
            .into(),
        attachments_index: bool_index(c.has_attachments),
        cloud_index: bool_index(c.has_cloud_links),
        override_new: r.override_new_recipients,
    }
}
fn show(app: &AppWindow, config: &Config) {
    if app.get_ai_policy_form().id.is_empty() {
        app.set_ai_policy_form(PolicyRuleForm {
            mode_index: 1,
            ..Default::default()
        });
    }
    app.set_ai_policy_config_json(serde_json::to_string(config).unwrap_or_default().into());
    let mut rows = vec![slint::SharedString::from("Nova regra / New rule")];
    rows.extend(config.rules.iter().map(|r| r.id.clone().into()));
    app.set_ai_policy_rules(ModelRc::new(VecModel::from(rows)));
    if let Ok(a) = action(app.get_ai_policy_action()) {
        app.set_ai_policy_mode(effect_index(
            config
                .defaults
                .get(&a)
                .copied()
                .unwrap_or_else(|| a.default_effect()),
        ));
    }
}
fn config(app: &AppWindow) -> Config {
    serde_json::from_str(&app.get_ai_policy_config_json()).unwrap_or_default()
}
fn action_label(a: Action) -> &'static str {
    match a {
        Action::List => "Listar / List",
        Action::Read => "Ler / Read",
        Action::Search => "Pesquisar / Search",
        Action::MarkRead => "Marcar leitura / Mark read",
        Action::Send => "Enviar / Send",
        Action::Reply => "Responder / Reply",
        Action::Delete => "Lixeira / Trash",
        Action::PermanentDelete => "Excluir permanentemente / Permanently delete",
        Action::CloudUpload => "Upload na nuvem / Cloud upload",
        Action::Download => "Baixar anexo / Download",
        Action::Sync => "Sincronizar / Sync",
        Action::Control => "Revisão e configurações / Review and settings",
    }
}
fn effect_label(e: Effect) -> &'static str {
    match e {
        Effect::Allow => "Permitir / Allow",
        Effect::Ask => "Perguntar / Ask",
        Effect::Block => "Bloquear / Block",
    }
}
fn result_label(result: &str) -> &str {
    match result {
        "started" => "Iniciada / Started",
        "pending_confirmation" => "Aguardando aprovação / Pending approval",
        "approved" => "Aprovada / Approved",
        "queued" => "Enfileirada / Queued",
        "succeeded" => "Concluída / Succeeded",
        "cancelled" => "Cancelada / Cancelled",
        "blocked" | "blocked_at_approval" | "blocked_during_upload" => "Bloqueada / Blocked",
        "failed" | "invalid_input" | "invalid_at_approval" => "Falhou / Failed",
        "expired" => "Expirada / Expired",
        "policy_change_requested" => "Alteração de regras solicitada / Policy change requested",
        "policy_updated" => "Regras salvas / Rules saved",
        "approved_result_retrieved" => "Resultado aprovado consultado / Approved result retrieved",
        "result_access_blocked" => "Acesso ao resultado bloqueado / Result access blocked",
        _ => result,
    }
}
fn actor_label(actor: &str) -> &str {
    match actor {
        "mcp_client" => "Cliente MCP / MCP client",
        "desktop_approval" => "Aprovação no aplicativo / App approval",
        "desktop_user" => "Usuário no aplicativo / App user",
        "desktop_compose" => "Compositor / Compose",
        "desktop_settings" => "Configurações / Settings",
        "mcp_confirmation" => "Confirmação MCP / MCP confirmation",
        "system" => "Sistema / System",
        _ => actor,
    }
}
fn history(events: Vec<mail_mcp::policy::Event>) -> String {
    use chrono::TimeZone;
    if events.is_empty() {
        return "Nenhuma ação registrada / No actions recorded".into();
    }
    events.into_iter().rev().map(|event| {
        let time=chrono::Local.timestamp_millis_opt(event.timestamp_ms).single()
            .map(|value|value.format("%Y-%m-%d %H:%M:%S %:z").to_string()).unwrap_or_default();
        let rules=event.decision.applied.iter().map(|rule|rule.rule_id.as_str()).collect::<Vec<_>>().join(", ");
        let actor=actor_label(&event.actor);
        let action=action_label(event.action);
        let result=result_label(&event.result);
        let effect=effect_label(event.decision.effect);
        format!("{time}\n{actor} · {action} · {result}\nRegra / Rule: {rules} · {effect} · revisão / revision {}\nContas / Accounts: {:?}; destinatários / recipients: {}; novos / new: {}\n", event.decision.revision,event.accounts,event.recipient_count,event.new_recipient_count)
    }).collect::<Vec<_>>().join("\n")
}
fn edited_config(app: &AppWindow, command: &str) -> Result<Option<Config>, String> {
    let mut config = config(app);
    match command {
        "default" => {
            config.defaults.insert(
                action(app.get_ai_policy_action())?,
                effect(app.get_ai_policy_mode()),
            );
        }
        "rule" => {
            let rule = form_rule(&app.get_ai_policy_form())?;
            if let Some(existing) = config.rules.iter_mut().find(|r| r.id == rule.id) {
                *existing = rule;
            } else {
                config.rules.push(rule);
            }
        }
        "remove" => {
            let id = app.get_ai_policy_form().id;
            config.rules.retain(|r| r.id != id.as_str());
        }
        "reset" => {
            config = Config::default();
        }
        _ => return Ok(None),
    }
    config
        .validate()
        .map_err(|e| format!("Regra inválida / Invalid rule: {e}"))?;
    Ok(Some(config))
}
pub(crate) fn register(app: &AppWindow, state: &Rc<RefCell<InboxState>>, runtime: &Handle) {
    let weak = app.as_weak();
    app.on_ai_policy_select(move |index| {
        if let Some(app) = weak.upgrade()
            && let Ok(a) = action(index)
        {
            let cfg = config(&app);
            app.set_ai_policy_mode(effect_index(
                cfg.defaults
                    .get(&a)
                    .copied()
                    .unwrap_or_else(|| a.default_effect()),
            ));
        }
    });
    let weak = app.as_weak();
    app.on_ai_policy_edit(move |index| {
        if let Some(app) = weak.upgrade() {
            let cfg = config(&app);
            let form = usize::try_from(index - 1)
                .ok()
                .and_then(|i| cfg.rules.get(i))
                .map(rule_form)
                .unwrap_or_else(|| PolicyRuleForm {
                    mode_index: 1,
                    ..Default::default()
                });
            app.set_ai_policy_form(form);
        }
    });
    let state = Rc::downgrade(state);
    let weak = app.as_weak();
    let runtime = runtime.clone();
    app.on_ai_policy_command(move |command| {
        let Some(app) = weak.upgrade() else { return; };
        if app.get_ai_policy_busy() { return; }
        let Some(state) = state.upgrade() else { return; };
        let Some(source) = state.borrow().core.clone() else {
            app.set_ai_policy_status("Conecte uma conta primeiro / Connect a mail account first".into());
            return;
        };
        let desired = match edited_config(&app, command.as_str()) {
            Ok(config) => config,
            Err(error) => { app.set_ai_policy_status(error.into()); return; }
        };
        app.set_ai_policy_busy(true);
        app.set_ai_policy_status("Carregando / Loading...".into());
        let weak = weak.clone();
        runtime.spawn(async move {
            let server = source.ai_server();
            let _lease = source;
            let result = async {
                let (config, fallback) = if let Some(config) = desired {
                    (server.save_policy(config).await?, false)
                } else {
                    server.policy_config().await
                };
                let events = server.action_history().await?;
                Ok::<_, anyhow::Error>((config, fallback, history(events)))
            }.await;
            let _ = weak.upgrade_in_event_loop(move |app| {
                app.set_ai_policy_busy(false);
                match result {
                    Ok((config, fallback, events)) => {
                        show(&app, &config);
                        app.set_ai_policy_history(events.into());
                        let status = if fallback {
                            "Arquivo inválido: padrões seguros ativos. Salvar substituirá o arquivo. / Invalid file: safe defaults active; Save replaces it."
                        } else {
                            "Regras carregadas; alterações valem na próxima ação e na aprovação. / Rules apply to the next action and approval."
                        };
                        app.set_ai_policy_status(status.into());
                    }
                    Err(error) => app.set_ai_policy_status(format!("Não foi possível salvar/carregar / Could not save/load: {error:#}").into()),
                }
            });
        });
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rule_form_round_trip() {
        let r = Rule {
            id: "safe.contacts".into(),
            action: Action::Reply,
            effect: Effect::Allow,
            conditions: Conditions {
                known_contacts: Some(true),
                accounts: vec![1],
                addresses: vec!["alice@example.invalid".into()],
                max_recipients: Some(3),
                has_cloud_links: Some(false),
                ..Default::default()
            },
            override_new_recipients: false,
        };
        let restored = form_rule(&rule_form(&r)).unwrap();
        assert_eq!(
            serde_json::to_value(r).unwrap(),
            serde_json::to_value(restored).unwrap()
        );
    }
    #[test]
    fn invalid_form_counts_are_rejected() {
        let f = PolicyRuleForm {
            id: "bad".into(),
            max_count: "not a number".into(),
            ..Default::default()
        };
        assert!(form_rule(&f).is_err());
    }
}
