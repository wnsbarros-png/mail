//! Native, app-lifetime daily scheduling. No background service, no autonomous retries.
use crate::{AppWindow, InboxState, TriageRow};

use flectar_mail_core::triage::{self, CATEGORIES, Digest, Settings};

use serde_json::json;

use slint::{ComponentHandle, ModelRc, VecModel};

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tokio::runtime::Handle;

fn show(app: &AppWindow, d: &Digest) {
    let filter = usize::try_from(app.get_digest_filter()).unwrap_or_default();

    let rows = d
        .items
        .iter()
        .filter(|i| filter == 0 || CATEGORIES.get(filter - 1) == Some(&i.category))
        .take(200)
        .map(|i| TriageRow {
            id: i.message_id as i32,
            sender: i.sender.clone().into(),
            subject: i.subject.clone().into(),
            category: i.category.label().into(),
            category_index: CATEGORIES
                .iter()
                .position(|c| *c == i.category)
                .unwrap_or(5) as i32,
            unread: i.unread,
        })
        .collect::<Vec<_>>();

    app.set_digest_rows(ModelRc::new(VecModel::from(rows)));

    app.set_digest_summary(
        format!(
            "{} · {} mensagens / messages · {}\n{}",
            d.start,
            d.items.len(),
            d.timezone,
            CATEGORIES
                .iter()
                .map(|c| format!(
                    "{}: {}",
                    c.label(),
                    d.counts.get(c).copied().unwrap_or_default()
                ))
                .collect::<Vec<_>>()
                .join(" · ")
        )
        .into(),
    );
}

fn settings(app: &AppWindow) -> anyhow::Result<Settings> {
    let account = app.get_digest_account();

    let s = Settings {
        time: app.get_digest_time().trim().into(),
        email_to_self: app.get_digest_email_self(),
        account_id: if account.trim().is_empty() {
            None
        } else {
            Some(account.trim().parse()?)
        },
        ai_enhancement: app.get_digest_enhance(),
        ..Default::default()
    };

    s.validate()?;

    Ok(s)
}

fn decode(v: serde_json::Value) -> anyhow::Result<Digest> {
    if v["status"] == "pending_confirmation" {
        anyhow::bail!(
            "Aguardando aprovação no aplicativo / Waiting for in-app approval; refresh after approval"
        );
    }

    Ok(serde_json::from_value(v)?)
}

type ReadTicket = Arc<Mutex<Option<(String, Option<i64>, bool, String)>>>;
async fn read_digest(
    source: &crate::mail::CoreMailSource,
    date: &str,
    settings: &Settings,
    ticket: &ReadTicket,
) -> anyhow::Result<Digest> {
    let previous = ticket
        .lock()
        .map_err(|_| anyhow::anyhow!("Digest state unavailable"))?
        .clone();
    let server = source.ai_server();
    if let Some((saved_date, account, enhance, id)) = previous
        && saved_date == date
        && account == settings.account_id
        && enhance == settings.ai_enhancement
    {
        let value = server
            .call_tool_from_app("get_action_result", &json!({"pendingId":id}))
            .await?;
        if value["status"] == "completed" {
            *ticket
                .lock()
                .map_err(|_| anyhow::anyhow!("Digest state unavailable"))? = None;
            return decode(value["result"].clone());
        }
        let still_pending = server
            .pending_actions()
            .await
            .iter()
            .any(|v| v["pendingId"] == id);
        if !still_pending {
            *ticket
                .lock()
                .map_err(|_| anyhow::anyhow!("Digest state unavailable"))? = None;
        }
        anyhow::bail!(
            "Aguardando aprovação, ou resultado expirou/foi cancelado; feche o resumo e revise o painel. / Awaiting approval, or result expired/cancelled; close the digest and review the panel."
        );
    }
    let args =
        json!({"date":date,"accountId":settings.account_id,"enhance":settings.ai_enhancement});
    let value = server.call_tool_from_app("get_digest", &args).await?;
    if let Some(id) = value.get("pendingId").and_then(|v| v.as_str()) {
        *ticket
            .lock()
            .map_err(|_| anyhow::anyhow!("Digest state unavailable"))? = Some((
            date.into(),
            settings.account_id,
            settings.ai_enhancement,
            id.into(),
        ));
    }
    decode(value)
}
async fn load(
    source: &crate::mail::CoreMailSource,
    date: String,
    desired: Option<Settings>,
    ticket: &ReadTicket,
) -> anyhow::Result<(Digest, Settings)> {
    let core = source.file_core();
    if let Some(s) = desired {
        if let Some(id) = s.account_id
            && !core.list_accounts().await?.iter().any(|a| a.id == id)
        {
            anyhow::bail!("Conta inexistente / Unknown account");
        }
        core.save_digest_settings(s).await?;
    }
    let settings = core.digest_settings().await?;
    let digest = read_digest(source, &date, &settings, ticket).await?;
    Ok((digest, settings))
}
async fn daily_tick(
    source: &crate::mail::CoreMailSource,
    ticket: &ReadTicket,
) -> anyhow::Result<Option<(Digest, &'static str)>> {
    let core = source.file_core();
    let s = core.digest_settings().await?;
    if !triage::due(chrono::Local::now(), &s) {
        return Ok(None);
    }
    let day = triage::today();
    if !core.claim_daily_digest(day.clone()).await? {
        return Ok(None);
    }
    let server = source.ai_server();
    let d = read_digest(source, &day, &s, ticket).await?;
    let status = if s.email_to_self {
        let id = s
            .account_id
            .ok_or_else(|| anyhow::anyhow!("Missing account"))?;
        match server.propose_digest_email(id, &d).await {
            Ok(v) if v["status"] == "pending_confirmation" => {
                "Envio do resumo aguardando aprovação / Digest email awaiting approval"
            }
            Ok(_) => "Resumo enfileirado conforme a política / Digest queued according to policy",
            Err(_) => {
                "Resumo local pronto; envio bloqueado ou falhou. Sem nova tentativa automática. / Local digest ready; email blocked or failed. No automatic retry."
            }
        }
    } else {
        "Resumo local pronto / Local digest ready"
    };
    Ok(Some((d, status)))
}
fn register_commands(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &Handle,
    ticket: ReadTicket,
) {
    let weak = app.as_weak();
    let state_weak = Rc::downgrade(state);
    let handle = runtime.clone();
    app.on_digest_command(move |command| {
        let Some(app)=weak.upgrade() else {return;};
        if app.get_digest_busy() {return;}
        let Some(state)=state_weak.upgrade() else {return;};
        let Some(source)=state.borrow().core.clone() else {
            app.set_digest_status("Conecte uma conta primeiro / Connect a mail account first".into());
            return;
        };
        let desired=if command=="save" {
            match settings(&app) {
                Ok(s)=>Some(s),
                Err(_)=>{
                    app.set_digest_status("Confira horário e ID da conta / Check time and account ID".into());
                    return;
                }
            }
        } else {None};
        let initial=command=="load";
        let date=if app.get_digest_date().is_empty() {triage::today()} else {app.get_digest_date().to_string()};
        let weak=weak.clone();
        let ticket=ticket.clone();
        app.set_digest_busy(true);
        handle.spawn(async move {
            let result=load(&source,date,desired,&ticket).await;
            let _lease=source;
            let _=weak.upgrade_in_event_loop(move |app| {
                app.set_digest_busy(false);
                match result {
                    Ok((d,s))=>{
                        app.set_digest_date(d.start.clone().into());
                        if initial {
                            app.set_digest_time(s.time.into());
                            app.set_digest_email_self(s.email_to_self);
                            app.set_digest_enhance(s.ai_enhancement);
                            app.set_digest_account(s.account_id.map(|i|i.to_string()).unwrap_or_default().into());
                        }
                        show(&app,&d);
                        let status=if d.truncated {"Limite de 5000 mensagens atingido / 5000-message limit reached"}
                        else {"Até 200 itens por filtro; correções valem para o remetente nesta conta. / Up to 200 items per filter; corrections apply to this sender in this account."};
                        app.set_digest_status(status.into());
                    }
                    Err(e)=>app.set_digest_status(format!("{e:#}").into())
                }
            });
        });
    });
}
fn register_corrections(app: &AppWindow, state: &Rc<RefCell<InboxState>>, runtime: &Handle) {
    let weak = app.as_weak();
    let state_weak = Rc::downgrade(state);
    let handle = runtime.clone();
    app.on_digest_correct(move |id,index| {
        let Some(app)=weak.upgrade() else {return;};
        if app.get_digest_busy() {return;}
        let Some(category)=usize::try_from(index).ok().and_then(|i|CATEGORIES.get(i).copied()) else {return;};
        let Some(state)=state_weak.upgrade() else {return;};
        let Some(source)=state.borrow().core.clone() else {return;};
        app.set_digest_busy(true);
        let weak=weak.clone();
        handle.spawn(async move {
            let server=source.ai_server();
            let args=json!({"messageId":id,"category":category});
            let result=server.call_tool_from_app("set_category",&args).await;
            let _lease=source;
            let _=weak.upgrade_in_event_loop(move |app| {
                app.set_digest_busy(false);
                match result {
                    Ok(v) if v["status"]=="pending_confirmation"=>app.set_digest_status(
                        "Correção proposta: feche o resumo e aprove no painel do aplicativo; depois atualize. / Correction proposed: close the digest and approve in the app review panel, then refresh.".into()),
                    Ok(_)=>app.invoke_digest_command("refresh".into()),
                    Err(e)=>app.set_digest_status(format!("{e:#}").into())
                }
            });
        });
    });
}
pub(crate) fn register(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &Handle,
) -> slint::Timer {
    let ticket: ReadTicket = Arc::new(Mutex::new(None));
    register_commands(app, state, runtime, ticket.clone());
    register_corrections(app, state, runtime);
    let weak = app.as_weak();
    let state_weak = Rc::downgrade(state);
    let handle = runtime.clone();
    let flight = Arc::new(AtomicBool::new(false));
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated,Duration::from_secs(30),move || {
        let Some(state)=state_weak.upgrade() else {return;};
        let Some(source)=state.borrow().core.clone() else {return;};
        if flight.swap(true,Ordering::AcqRel) {return;}
        let flight=flight.clone();
        let ticket=ticket.clone();
        let weak=weak.clone();
        handle.spawn(async move {
            let result=daily_tick(&source,&ticket).await;
            flight.store(false,Ordering::Release);
            let _lease=source;
            let _=weak.upgrade_in_event_loop(move |app| {
                match result {
                    Ok(Some((d,status)))=>{
                        if !app.get_digest_busy()&&!app.get_digest_open() {
                            app.set_digest_date(d.start.clone().into());
                            show(&app,&d);
                        }
                        app.set_digest_status(status.into());
                        notify();
                    }
                    Ok(None)=>{},
                    Err(_)=>app.set_digest_status(
                        "Resumo agendado indisponível: verifique a política e atualize manualmente. Sem nova tentativa automática hoje. / Scheduled digest unavailable: check policy and refresh manually. No automatic retry today.".into())
                }
            });
        });
    });
    timer
}

fn notify() {
    #[cfg(desktop)]
    {
        if notify_rust::Notification::new()
            .summary("Resumo do dia / Daily digest")
            .body(
                "Seu resumo está disponível no aplicativo / Your local digest is ready in the app",
            )
            .show()
            .is_err()
        {
            tracing::debug!("Daily digest notification unavailable; view remains available");
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn pending_is_not_a_digest() {
        assert!(
            decode(json!({
            "status":"pending_confirmation","pendingId":"synthetic"}
            ))
            .is_err()
        );
    }

    #[test]
    fn categories_remain_stable() {
        assert_eq!(CATEGORIES.len(), 6);

        assert_eq!(CATEGORIES[0], triage::Category::Urgent);
    }
    async fn source() -> (crate::mail::CoreMailSource, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let source = crate::mail::CoreMailSource::start(
            flectar_mail_core::config::Paths::for_tests(dir.path()),
            Arc::new(flectar_mail_core::accounts::credentials::UnavailableCredentialStore),
            Arc::new(flectar_mail_core::oauth::redirect::LoopbackRedirectBroker::default()),
        )
        .await
        .unwrap();
        (source, dir)
    }
    #[tokio::test]
    async fn native_read_ask_reuses_one_proposal_then_approved_result() {
        use mail_mcp::policy::{Action, Effect};
        let (source, _dir) = source().await;
        let server = source.ai_server();
        let mut config = server.policy_config().await.0;
        config.defaults.insert(Action::Read, Effect::Ask);
        server.save_policy(config).await.unwrap();
        let ticket: ReadTicket = Arc::new(Mutex::new(None));
        let day = triage::today();
        let settings = Settings::default();
        assert!(
            read_digest(&source, &day, &settings, &ticket)
                .await
                .is_err()
        );
        assert!(
            read_digest(&source, &day, &settings, &ticket)
                .await
                .is_err()
        );
        assert_eq!(server.pending_actions().await.len(), 1);
        let pending = server.pending_actions().await;
        server
            .approve_in_app(pending[0]["pendingId"].as_str().unwrap())
            .await
            .unwrap();
        let result = read_digest(&source, &day, &settings, &ticket)
            .await
            .unwrap();
        assert!(result.items.is_empty());
        assert!(ticket.lock().unwrap().is_none());
        config = server.policy_config().await.0;
        config.defaults.insert(Action::Read, Effect::Block);
        server.save_policy(config).await.unwrap();
        assert!(
            read_digest(&source, &day, &settings, &ticket)
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn native_scheduler_claim_survives_settings_changes_without_email() {
        let (source, _dir) = source().await;
        let core = source.file_core();
        core.save_digest_settings(Settings {
            time: "00:00".into(),
            ..Default::default()
        })
        .await
        .unwrap();
        let ticket: ReadTicket = Arc::new(Mutex::new(None));
        assert!(daily_tick(&source, &ticket).await.unwrap().is_some());
        assert!(daily_tick(&source, &ticket).await.unwrap().is_none());
        assert!(source.ai_server().pending_actions().await.is_empty());
        core.save_digest_settings(Settings {
            time: "00:01".into(),
            ..Default::default()
        })
        .await
        .unwrap();
        assert!(daily_tick(&source, &ticket).await.unwrap().is_none());
    }
}
