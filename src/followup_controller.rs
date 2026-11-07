//! Local reminder timer: no send calls; proposals require an explicit UI click.
use crate::ui_message::AppWindowMessages;
use crate::{AppWindow, FollowupRow, InboxState};
use flectar_mail_core::followups::Followup;
use serde_json::{Value, json};
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
type Ticket = Arc<Mutex<Option<String>>>;
async fn rows(
    source: &crate::mail::CoreMailSource,
    ticket: &Ticket,
) -> anyhow::Result<Vec<Followup>> {
    let server = source.ai_server();
    let previous = ticket
        .lock()
        .map_err(|_| anyhow::anyhow!("Reminder state unavailable"))?
        .clone();
    let value = if let Some(id) = previous {
        let value = server
            .call_tool_from_app("get_action_result", &json!({"pendingId":id}))
            .await?;
        if value["status"] == "completed" {
            *ticket
                .lock()
                .map_err(|_| anyhow::anyhow!("Reminder state unavailable"))? = None;
            value["result"].clone()
        } else {
            let pending = server
                .pending_actions()
                .await
                .iter()
                .any(|v| v["pendingId"] == id);
            if !pending {
                *ticket
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Reminder state unavailable"))? = None;
            }
            anyhow::bail!(
                "Feche os lembretes e aprove no painel; depois atualize. / Close reminders and approve in the panel, then refresh."
            );
        }
    } else {
        server
            .call_tool_from_app("list_followups", &json!({}))
            .await?
    };
    if let Some(id) = value["pendingId"].as_str() {
        *ticket
            .lock()
            .map_err(|_| anyhow::anyhow!("Reminder state unavailable"))? = Some(id.into());
        anyhow::bail!(
            "Feche os lembretes e aprove a leitura no painel. / Close reminders and approve the read in the panel."
        );
    }
    let value = value.get("result").cloned().unwrap_or(value);
    Ok(serde_json::from_value(value)?)
}
fn present(app: &AppWindow, rows: Vec<Followup>) {
    let now = chrono::Utc::now().timestamp_millis();
    app.set_followup_rows(ModelRc::new(VecModel::from(
        rows.into_iter()
            .map(|f| FollowupRow {
                id: f.message_id as i32,
                subject: f.subject.into(),
                due: chrono::DateTime::from_timestamp_millis(f.due_ms)
                    .map(|d| {
                        d.with_timezone(&chrono::Local)
                            .format("%d/%m/%Y %H:%M %:z")
                            .to_string()
                    })
                    .unwrap_or_default()
                    .into(),
                state: match f.state.as_str() {
                    "answered" => "Respondido / Answered",
                    "cancelled" => "Cancelado / Cancelled",
                    "drafted" => "Proposta preparada / Proposal prepared",
                    _ if f.due_ms <= now => "Resposta pendente / Reply overdue",
                    _ => "Aguardando resposta / Waiting for reply",
                }
                .into(),
                active: matches!(f.state.as_str(), "waiting" | "notified" | "drafted"),
                can_propose: f.due_ms <= now && matches!(f.state.as_str(), "waiting" | "notified"),
            })
            .collect::<Vec<_>>(),
    )));
}
pub(crate) fn register(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &Handle,
) -> slint::Timer {
    let ticket: Ticket = Arc::new(Mutex::new(None));
    let weak = app.as_weak();
    let state_weak = Rc::downgrade(state);
    let handle = runtime.clone();
    app.on_followup_command(move|command,id|{
  let Some(app)=weak.upgrade()else{return;};if app.get_followup_busy(){return;}
  let Some(state)=state_weak.upgrade()else{return;};
  if command=="selected"{let state=state.borrow();let selected=if state.conversation_owner_id==state.selected_id{state.conversation_messages.get(state.conversation_selected_index).and_then(|m|m.message_id)}else{state.messages.iter().find(|m|Some(m.id)==state.selected_id).and_then(|m|m.message_id)};app.set_followup_message(selected.map(|m|m.to_string()).unwrap_or_default().into());return;}
  let Some(source)=state.borrow().core.clone()else{return;};
  let message=app.get_followup_message().to_string();let days=app.get_followup_days().to_string();let template=app.get_followup_template().to_string();let weak=weak.clone();let ticket=ticket.clone();app.set_followup_busy(true);
  handle.spawn(async move{
   let result=async{let server=source.ai_server();let value=match command.as_str(){"set"=>{let message:i64=message.trim().parse()?;let days:i64=days.trim().parse()?;let template=if template.trim().is_empty(){None}else{Some(template.trim().parse::<i64>()?)};server.call_tool_from_app("set_followup",&json!({"messageId":message,"days":days,"template_id":template})).await?},"cancel"=>server.call_tool_from_app("cancel_followup",&json!({"messageId":id})).await?,"propose"=>server.propose_followup(i64::from(id)).await?,_=>Value::Null};if value["status"]=="pending_confirmation"{return Ok::<_,anyhow::Error>((None,true));}Ok((Some(rows(&source,&ticket).await?),false))}.await;
   let _lease=source;let _=weak.upgrade_in_event_loop(move|app|{app.set_followup_busy(false);match result{Ok((_,true))=>{app.set_followup_status("Proposta pendente: feche esta tela e aprove no painel do aplicativo; depois atualize. / Proposal pending: close this screen and approve in the app panel, then refresh.".into());app.set_followup_open(false);},Ok((Some(rows),false))=>{present(&app,rows);app.set_followup_status("Até 200 lembretes; nenhum envio automático. / Up to 200 reminders; no automatic send.".into());},Ok(_)=>{},Err(e)=>app.set_followup_status(format!("{e:#}").into())}});
  });
 });
    let weak = app.as_weak();
    let state_weak = Rc::downgrade(state);
    let handle = runtime.clone();
    let flight = Arc::new(AtomicBool::new(false));
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated,Duration::from_secs(30),move||{let Some(state)=state_weak.upgrade()else{return;};let Some(source)=state.borrow().core.clone()else{return;};if flight.swap(true,Ordering::AcqRel){return;}let weak=weak.clone();let flight=flight.clone();handle.spawn(async move{let result=source.file_core().claim_followup_notices().await;flight.store(false,Ordering::Release);let _lease=source;if let Ok(count)=result&&count>0{let _=weak.upgrade_in_event_loop(move|app|{app.set_followup_status("Há respostas pendentes. / Replies are overdue.".into());app.set_sync_status(crate::ui_message::UiMessage::plain("Follow-up reminders are due. Open Settings, Follow-up reminders."));if notify_rust::Notification::new().summary("Lembretes de resposta / Follow-up reminders").body("Há respostas pendentes. Confira os lembretes no aplicativo. / Replies are overdue. Review reminders in the app.").show().is_err(){tracing::debug!("Follow-up OS notification unavailable");}});}});});
    timer
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn native_read_ask_reuses_one_ticket_and_never_sends() {
        use mail_mcp::policy::{Action, Effect};
        let dir = tempfile::tempdir().unwrap();
        let source = crate::mail::CoreMailSource::start(
            flectar_mail_core::config::Paths::for_tests(dir.path()),
            Arc::new(flectar_mail_core::accounts::credentials::UnavailableCredentialStore),
            Arc::new(flectar_mail_core::oauth::redirect::LoopbackRedirectBroker::default()),
        )
        .await
        .unwrap();
        let server = source.ai_server();
        let mut config = server.policy_config().await.0;
        config.defaults.insert(Action::Read, Effect::Ask);
        server.save_policy(config).await.unwrap();
        let ticket: Ticket = Arc::new(Mutex::new(None));
        assert!(rows(&source, &ticket).await.is_err());
        assert!(rows(&source, &ticket).await.is_err());
        let pending = server.pending_actions().await;
        assert_eq!(pending.len(), 1);
        server
            .approve_in_app(pending[0]["pendingId"].as_str().unwrap())
            .await
            .unwrap();
        assert!(rows(&source, &ticket).await.unwrap().is_empty());
        assert!(ticket.lock().unwrap().is_none());
        assert!(server.pending_actions().await.is_empty());
    }
}
