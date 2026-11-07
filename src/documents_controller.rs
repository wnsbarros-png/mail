//! Native, policy-gated local extraction. No network or automatic writes outside the index.
use crate::{AppWindow, DocumentRow, InboxState};
use flectar_mail_core::documents::{DocumentRecord, Extraction, FinancialDocument};
use serde_json::{Value, json};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};
use tokio::runtime::Handle;
type Ticket = Arc<Mutex<Option<(String, String)>>>;
pub(crate) fn open(app: &AppWindow, state: &Rc<RefCell<InboxState>>) {
    let state = state.borrow();
    let id = if state.conversation_owner_id == state.selected_id {
        state
            .conversation_messages
            .get(state.conversation_selected_index)
            .and_then(|m| m.message_id)
    } else {
        state
            .messages
            .iter()
            .find(|m| Some(m.id) == state.selected_id)
            .and_then(|m| m.message_id)
    };
    app.set_documents_message(id.map(|n| n.to_string()).unwrap_or_default().into());
    app.set_documents_rows(ModelRc::new(VecModel::from(Vec::<DocumentRow>::new())));
    app.set_documents_status(
        "Somente dados locais; clique em extrair. / Cached files only; click Extract.".into(),
    );
    app.set_documents_open(true);
}
pub(crate) fn warning(s: &str) -> String {
    match s{
 "not_authenticated"=>"Autenticidade não verificada / Authenticity not verified",
 "xml_signature_not_verified"|"event_signature_not_verified"=>"Assinatura XML não validada / XML signature not validated",
 "invalid_access_key"|"invalid_dv"=>"DV inválido / Invalid check digit",
 "invalid_crc16"=>"CRC PIX inválido / Invalid PIX CRC",
 "overdue"=>"Vencido (data inferida ou declarada) / Overdue (inferred or declared date)",
 "body_amount_mismatch"|"printed_amount_mismatch"=>"Valor divergente do corpo ou texto / Amount differs from body or printed text",
 "body_amount_ambiguous"=>"Vários valores no corpo; comparação inconclusiva / Multiple body amounts; comparison inconclusive",
 "due_factor_cycle_ambiguous"=>"Fator com dois ciclos; confira a data / Factor has two cycles; verify date",
 "due_date_inferred"=>"Vencimento inferido pela data da mensagem / Due date inferred using mail date",
 "printed_due_factor_mismatch"=>"Data impressa diverge do fator / Printed date conflicts with factor",
 "receipt_is_heuristic_not_payment_confirmation"=>"Extração heurística: NÃO comprova pagamento / Heuristic extraction: NOT proof of payment",
 "pix_recipient_not_verified"=>"Recebedor PIX não verificado / PIX recipient not verified",
 "reference_value_not_currency"=>"Valor de referência, não monetário / Reference value, not currency",
 "no_authorization_protocol"=>"Sem protocolo de autorização / No authorization protocol",
 "protocol_key_mismatch"|"access_key_fields_mismatch"=>"Chave diverge dos campos ou protocolo / Key differs from fields or protocol",
 _=>s}.into()
}
fn summary(d: &FinancialDocument) -> String {
    let party = |p: &Option<flectar_mail_core::documents::Party>| {
        p.as_ref()
            .map(|p| {
                format!(
                    "{} {}",
                    p.name.as_deref().unwrap_or("—"),
                    p.cnpj.as_deref().unwrap_or_default()
                )
            })
            .unwrap_or_else(|| "—".into())
    };
    let amount = d
        .amount_cents
        .map(|v| format!("R$ {},{:02}", v / 100, v % 100))
        .unwrap_or_else(|| "—".into());
    let mut s = format!(
        "Emitente/Pagador · Issuer/Payer: {}\nDestinatário/Beneficiário · Recipient: {}\nNúmero/Série · Number/Series: {} / {}  |  Emissão · Issued: {}\nValor · Amount: {}  |  Vencimento · Due: {}\nProtocolo/Status declarados (não validados) · Claimed protocol/status: {} / {}\nConfiança · Confidence: {}  |  Itens · Items: {}",
        party(&d.issuer),
        party(&d.recipient),
        d.number.as_deref().unwrap_or("—"),
        d.series.as_deref().unwrap_or("—"),
        d.issued_at.as_deref().unwrap_or("—"),
        amount,
        d.due_date.as_deref().unwrap_or("—"),
        d.protocol.as_deref().unwrap_or("—"),
        d.status.as_deref().unwrap_or("—"),
        d.confidence,
        d.item_count
    );
    if let Some(key) = &d.access_key {
        s.push_str(&format!("\nChave · Access key: {key}"));
    }
    if let Some(code) = d.fields.get("bank_code") {
        s.push_str(&format!("\nBanco · Bank: {code}"));
    }
    if let Some(code) = &d.code {
        let preview = code.chars().take(64).collect::<String>();
        s.push_str(&format!(
            "\nCódigo · Code: {preview}{}",
            if code.chars().count() > 64 { "…" } else { "" }
        ));
    }
    if !d.items.is_empty() {
        s.push_str(&format!(
            "\nItens · Items: {}",
            d.items
                .iter()
                .take(3)
                .map(|i| i.description.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    if !d.due_candidates.is_empty() {
        s.push_str(&format!(
            "\nDatas possíveis · Candidate dates: {}",
            d.due_candidates.join(" / ")
        ));
    }
    if let Some(id) = d
        .fields
        .get("e2e")
        .or_else(|| d.fields.get("transaction_id"))
    {
        s.push_str(&format!("\nID: {id}"));
    }
    s
}
pub(crate) fn present(app: &AppWindow, records: Vec<DocumentRecord>) {
    app.set_documents_rows(ModelRc::new(VecModel::from(
        records
            .into_iter()
            .map(|r| {
                let d = r.document;
                let summary = summary(&d);
                let code = d.access_key.clone().or(d.code.clone()).unwrap_or_default();
                DocumentRow {
                    title: format!("{} · {}", d.kind_name().to_uppercase(), r.filename).into(),
                    summary: summary.into(),
                    warnings: d
                        .warnings
                        .iter()
                        .map(|s| warning(s))
                        .collect::<Vec<_>>()
                        .join("; ")
                        .into(),
                    code: code.into(),
                    details: serde_json::to_string_pretty(&d).unwrap_or_default().into(),
                }
            })
            .collect::<Vec<_>>(),
    )));
}
async fn run(
    source: &crate::mail::CoreMailSource,
    ticket: &Ticket,
    tool: &str,
    args: &Value,
) -> anyhow::Result<(String, Value)> {
    let previous = ticket
        .lock()
        .map_err(|_| anyhow::anyhow!("Document state unavailable"))?
        .clone();
    let (tool, value) = if let Some((name, id)) = previous {
        let value = source
            .ai_server()
            .call_tool_from_app("get_action_result", &json!({"pendingId":id}))
            .await?;
        if value["status"] != "completed" {
            if !source
                .ai_server()
                .pending_actions()
                .await
                .iter()
                .any(|v| v["pendingId"] == id)
            {
                *ticket.lock().unwrap() = None;
            }
            anyhow::bail!(
                "Feche Documentos e aprove no painel; depois atualize. / Close Documents, approve in the panel, then refresh."
            );
        }
        *ticket.lock().unwrap() = None;
        (name, value["result"].clone())
    } else {
        (
            tool.into(),
            source.ai_server().call_tool_from_app(tool, args).await?,
        )
    };
    if value["status"] == "pending_confirmation" {
        if let Some(id) = value["pendingId"].as_str() {
            *ticket.lock().unwrap() = Some((tool, id.into()));
        }
        anyhow::bail!(
            "Aprovação pendente no painel do aplicativo. / Approval pending in the app panel."
        );
    }
    Ok((tool, value.get("result").cloned().unwrap_or(value)))
}
pub(crate) fn register(app: &AppWindow, state: &Rc<RefCell<InboxState>>, runtime: &Handle) {
    let clipboard = app.as_weak();
    app.on_documents_copy(move |text| {
        if let Some(app) = clipboard.upgrade() {
            app.set_clipboard_request(text);
            app.set_clipboard_request_id(app.get_clipboard_request_id().wrapping_add(1));
        }
    });
    let weak = app.as_weak();
    let state = Rc::downgrade(state);
    let handle = runtime.clone();
    let ticket: Ticket = Arc::new(Mutex::new(None));
    app.on_documents_command(move |command| {
        let Some(app) = weak.upgrade() else {
            return;
        };
        if app.get_documents_busy() {
            return;
        }
        let Some(state) = state.upgrade() else {
            return;
        };
        let Some(source) = state.borrow().core.clone() else {
            return;
        };
        let id = app.get_documents_message().to_string();
        let weak = weak.clone();
        let ticket = ticket.clone();
        app.set_documents_busy(true);
        handle.spawn(async move {
            let result =
                async {
                    let (tool, args) =
                        if command == "export" && ticket.lock().unwrap().is_none() {
                            let folder =
                                rfd::AsyncFileDialog::new().pick_folder().await.ok_or_else(
                                    || anyhow::anyhow!("Export cancelled / Exportação cancelada"),
                                )?;
                            (
                                "export_documents_csv",
                                json!({"folder":folder.path().to_string_lossy(),"filters":{}}),
                            )
                        } else {
                            (
                                "extract_documents",
                                json!({"message_id":id.parse::<i64>()?}),
                            )
                        };
                    run(&source, &ticket, tool, &args).await
                }
                .await;
            let _lease = source;
            let _ = weak.upgrade_in_event_loop(move |app| {
                app.set_documents_busy(false);
                if app.get_documents_message().as_str()!=id {app.set_documents_status("A mensagem selecionada mudou; extraia novamente. / Selected message changed; extract again.".into());return;}
                match result {
                    Ok((tool, value)) if tool == "export_documents_csv" => app
                        .set_documents_status(
                            format!("CSV: {}", value["path"].as_str().unwrap_or("—")).into(),
                        ),
                    Ok((_, value)) => match serde_json::from_value::<Extraction>(value) {
                        Ok(data) => {
                            if data.message_id.to_string()!=id {app.set_documents_status("A leitura anterior era de outra mensagem; extraia novamente. / Previous result was for another message; extract again.".into());return;}
                                    present(&app, data.documents);
                            app.set_documents_status(
                                format!(
                                    "{}{}",
                                    data.warnings.join("; "),
                                    if data.truncated {
                                        " · Limite atingido / Results limited"
                                    } else {
                                        ""
                                    }
                                )
                                .into(),
                            );
                        }
                        Err(_) => {
                            app.set_documents_status("Resposta inválida / Invalid result".into())
                        }
                    },
                    Err(e) => app.set_documents_status(format!("{e:#}").into()),
                }
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn approved_read_result_is_consumed_once_without_another_proposal() {
        let dir = tempfile::tempdir().unwrap();
        let source = crate::mail::CoreMailSource::start(
            flectar_mail_core::config::Paths::for_tests(dir.path()),
            Arc::new(flectar_mail_core::accounts::credentials::UnavailableCredentialStore),
            Arc::new(flectar_mail_core::oauth::redirect::LoopbackRedirectBroker::default()),
        )
        .await
        .unwrap();
        let server = source.ai_server();
        let mut cfg = server.policy_config().await.0;
        cfg.defaults.insert(
            mail_mcp::policy::Action::Read,
            mail_mcp::policy::Effect::Ask,
        );
        server.save_policy(cfg).await.unwrap();
        let ticket: Ticket = Arc::new(Mutex::new(None));
        assert!(
            run(&source, &ticket, "list_documents", &json!({}))
                .await
                .is_err()
        );
        assert!(
            run(&source, &ticket, "list_documents", &json!({}))
                .await
                .is_err()
        );
        let pending = server.pending_actions().await;
        assert_eq!(pending.len(), 1);
        server
            .approve_in_app(pending[0]["pendingId"].as_str().unwrap())
            .await
            .unwrap();
        let (tool, value) = run(&source, &ticket, "list_documents", &json!({}))
            .await
            .unwrap();
        assert_eq!(tool, "list_documents");
        assert_eq!(value, json!([]));
        assert!(ticket.lock().unwrap().is_none());
        assert!(server.pending_actions().await.is_empty());
    }
}
