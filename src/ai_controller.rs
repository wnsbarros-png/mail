//! UI-only approval entry point for the embedded MCP tool layer.
use crate::{AppWindow, ExternalUrlOpener, InboxState};
use serde_json::Value;
use slint::ComponentHandle;
use std::collections::HashSet;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::runtime::Handle;

pub(crate) fn register(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &Handle,
    opener: &ExternalUrlOpener,
) -> slint::Timer {
    register_cloud_auth(app, state, runtime, opener);
    crate::ai_policy_controller::register(app, state, runtime);
    let weak_state = Rc::downgrade(state);
    let weak_app = app.as_weak();
    let handle = runtime.clone();
    app.on_ai_review_action(move |id, approve| {
        let Some(state) = weak_state.upgrade() else {
            return;
        };
        let Some(source) = state.borrow().core.clone() else {
            return;
        };
        let server = source.ai_server();
        let weak_app = weak_app.clone();
        if let Some(app) = weak_app.upgrade() {
            if app.get_ai_review_busy() {
                return;
            }
            app.set_ai_review_busy(true);
            app.set_ai_review_status(if approve {
                "Processando aprovação...".into()
            } else {
                "Cancelando proposta...".into()
            });
        }
        handle.spawn(async move {
            // Keep the profile lease and listener alive for the whole operation.
            let _source = source;
            let result = if approve {
                server.approve_in_app(&id).await
            } else {
                server.cancel_in_app(&id).await
            };
            let _ = weak_app.upgrade_in_event_loop(move |app| {
                app.set_ai_review_busy(false);
                app.set_ai_review_status(match result {
                    Ok(_) => "".into(),
                    Err(error) => format!("{error:#}").into(),
                });
            });
        });
    });
    let weak_state = Rc::downgrade(state);
    let weak_app = app.as_weak();
    let handle = runtime.clone();
    let in_flight = Arc::new(AtomicBool::new(false));
    let settings_loaded = Arc::new(AtomicBool::new(false));
    let seen = Arc::new(std::sync::Mutex::new(HashSet::<String>::new()));
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(750),
        move || {
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            let Some(source) = state.borrow().core.clone() else {
                return;
            };
            let Some(app) = weak_app.upgrade() else {
                return;
            };
            let account_id = i64::from(app.get_compose_account_id());
            let notify_enabled = app.get_notifications_enabled();
            // Freeze the visible proposal while a decision is being processed.
            if in_flight.swap(true, Ordering::AcqRel) {
                return;
            }
            let weak_app = weak_app.clone();
            let in_flight = in_flight.clone();
            let seen = seen.clone();
            let settings_loaded = settings_loaded.clone();
            handle.spawn(async move {
                let server = source.ai_server();
                let actions = server.pending_actions().await;
                let progress = server.cloud_progress();
                let settings = source.file_core().cloud_settings();
                let notify = {
                    let mut seen = seen
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let notify = actions
                        .iter()
                        .filter_map(|a| a["pendingId"].as_str())
                        .any(|id| !seen.contains(id));
                    *seen = actions
                        .iter()
                        .filter_map(|a| a["pendingId"].as_str().map(String::from))
                        .collect();
                    notify
                };
                if notify && notify_enabled {
                    std::mem::drop(tokio::task::spawn_blocking(notify_pending_action));
                }

                let _ = weak_app.upgrade_in_event_loop(move |app| {
                    app.set_compose_cloud_limit(settings.mail_limit(account_id) as f32);
                    if !settings_loaded.swap(true, Ordering::AcqRel) {
                        app.set_dropbox_app_key(settings.dropbox_app_key.into());
                        if let Some(id) = settings.storage_account_id {
                            app.set_cloud_auth_account(id.to_string().into());
                        }
                    }
                    if app.get_ai_review_busy() {
                        if let (Some(name), Some(done), Some(total)) = (
                            progress["filename"].as_str(),
                            progress["done"].as_u64(),
                            progress["total"].as_u64(),
                        ) {
                            app.set_ai_review_status(
                                format!(
                                    "Enviando para {}: {} — {} / {} bytes",
                                    progress["provider"].as_str().unwrap_or_default(),
                                    visible(name),
                                    done,
                                    total
                                )
                                .into(),
                            );
                        }
                    } else {
                        app.set_ai_pending_count(actions.len() as i32);
                        if let Some(first) = actions.first() {
                            app.set_ai_pending_id(
                                first["pendingId"].as_str().unwrap_or_default().into(),
                            );
                            app.set_ai_pending_summary(readable_summary(&first["summary"]).into());
                        } else {
                            app.set_ai_pending_id("".into());
                            app.set_ai_pending_summary("".into());
                        }
                    }
                    in_flight.store(false, Ordering::Release);
                });
            });
        },
    );
    timer
}

/// OS notification contains no message, address, filename or bearer link.
fn notify_pending_action() {
    #[cfg(desktop)]
    if notify_rust::Notification::new()
        .summary("Ação aguardando aprovação")
        .body("Revise a proposta no aplicativo. Nenhum arquivo será enviado sem sua aprovação.")
        .show()
        .is_err()
    {
        tracing::debug!("Approval notification unavailable; in-app review remains visible");
    }
}

fn register_cloud_auth(
    app: &AppWindow,
    state: &Rc<RefCell<InboxState>>,
    runtime: &Handle,
    opener: &ExternalUrlOpener,
) {
    let state = Rc::downgrade(state);
    let weak = app.as_weak();
    let runtime = runtime.clone();
    let opener = opener.clone();
    app.on_cloud_connect(move |provider, account, key| {
        let Some(app) = weak.upgrade() else {
            return;
        };
        if app.get_cloud_auth_busy() {
            return;
        }
        let Some(state) = state.upgrade() else {
            return;
        };
        let Some(source) = state.borrow().core.clone() else {
            app.set_cloud_auth_status("Conecte primeiro uma conta de email.".into());
            return;
        };
        let core = source.file_core();
        let save_only = provider == "save";
        let kind = if save_only {
            flectar_mail_core::cloud_links::CloudProvider::Dropbox
        } else {
            match flectar_mail_core::cloud_links::CloudProvider::parse(&provider) {
                Ok(kind) => kind,
                Err(_) => return,
            }
        };
        let id = if kind == flectar_mail_core::cloud_links::CloudProvider::Dropbox {
            0
        } else {
            match account.parse::<i64>() {
                Ok(id) => id,
                Err(_) => {
                    app.set_cloud_auth_status("Informe o ID da conta Gmail ou Microsoft.".into());
                    return;
                }
            }
        };
        let key = key.to_string();
        app.set_cloud_auth_busy(true);
        app.set_cloud_auth_status(if save_only {
            "Salvando chave pública.".into()
        } else {
            "Conclua a autorização no navegador.".into()
        });
        let weak = weak.clone();
        let opener = opener.clone();
        runtime.spawn(async move {
            let _lease = source;
            let result = async {
                if kind == flectar_mail_core::cloud_links::CloudProvider::Dropbox {
                    core.set_dropbox_app_key(key).await?;
                }
                if !save_only {
                    core.connect_cloud(kind, id, move |url| opener.open_authorization(&url))
                        .await?;
                }
                Ok::<(), flectar_mail_core::error::CoreError>(())
            }
            .await;
            let _ = weak.upgrade_in_event_loop(move |app| {
                app.set_cloud_auth_busy(false);
                app.set_cloud_auth_status(match result {
                    Ok(()) => {
                        if save_only {
                            "Chave pública salva.".into()
                        } else {
                            "Armazenamento conectado. Tokens no chaveiro do sistema.".into()
                        }
                    }
                    Err(e) => format!("Não foi possível autorizar o armazenamento: {e}").into(),
                });
            });
        });
    });
}

fn visible(value: &str) -> String {
    value
        .chars()
        .flat_map(|c| {
            if c.is_control() || matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}') {
                format!("\\u{{{:x}}}", c as u32).chars().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}
fn addresses(value: &Value) -> String {
    match value {
        Value::String(v) => visible(v),
        Value::Array(v) => v.iter().map(addresses).collect::<Vec<_>>().join(", "),
        Value::Object(v) => {
            let email = v.get("email").and_then(Value::as_str).unwrap_or_default();
            let name = v.get("name").and_then(Value::as_str).unwrap_or_default();
            if name.is_empty() {
                visible(email)
            } else {
                format!("{} <{}>", visible(name), visible(email))
            }
        }
        _ => String::new(),
    }
}
fn readable_summary(s: &Value) -> String {
    let action = match s["action"].as_str().unwrap_or_default() {
        "send_cloud_links" => "Enviar email com links de arquivos",
        "send" => "Enviar email",
        "followup" => "Retorno de mensagem sem resposta / Follow-up for an unanswered message",
        "reply" => "Responder email",
        "reply_all" => "Responder a todos",
        "move_to_trash" => "Mover para a lixeira",
        "delete_permanently" => "Excluir permanentemente",
        _ => "Revisar ação",
    };
    let mut out = format!("{action}\n");
    if let Some(applied) = s["policy"]["applied"].as_array() {
        out.push_str(&format!(
            "Regra / Rule: {}\n",
            applied
                .iter()
                .filter_map(|r| r["rule_id"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if s["action"] == "tool_request" {
        out.push_str(&format!(
            "Operação / Operation: {}\nContas / Accounts: {}\n",
            visible(s["tool"].as_str().unwrap_or_default()),
            s["accountIds"]
        ));
        if s["tool"] == "set_category" {
            let category = s["request"].get("category").cloned().and_then(|v| {
                serde_json::from_value::<flectar_mail_core::triage::Category>(v).ok()
            });
            out.push_str(&format!(
                "Categoria / Category: {}\n",
                category.map(|c| c.label()).unwrap_or("Inválida / Invalid")
            ));
            out.push_str("Esta correção será lembrada para o remetente nesta conta, inclusive mensagens futuras. / Remember this sender correction in this account, including future messages.\n");
        }
        for key in [
            "messageId",
            "messageIds",
            "query",
            "folder",
            "filename",
            "read",
            "date",
            "start",
            "end",
            "enhance",
            "days",
            "template_id",
        ] {
            if let Some(value) = s["request"].get(key) {
                out.push_str(&format!("{key}: {}\n", visible(&value.to_string())));
            }
        }
        return out;
    }

    if s.get("messageIds").is_some() {
        out.push_str(&format!("Mensagens: {}\n", s["messageIds"]));
        return out;
    }
    out.push_str(&format!(
        "De: {} (conta {})\nPara: {}\n",
        addresses(&s["from"]),
        s["accountId"].as_i64().unwrap_or_default(),
        addresses(&s["to"])
    ));
    for (key, label) in [("cc", "Cc"), ("bcc", "Cco")] {
        let value = addresses(&s[key]);
        if !value.is_empty() {
            out.push_str(&format!("{label}: {value}\n"));
        }
    }
    out.push_str(&format!(
        "Assunto: {}\n",
        visible(s["subject"].as_str().unwrap_or_default())
    ));
    if s["anyoneWithLink"].as_bool() == Some(true) {
        out.push_str(&format!("\nUpload para: {} (conta de armazenamento {})\nATENÇÃO: qualquer pessoa com o link poderá baixar os arquivos.\nUpload somente após Aprovar. O email será colocado na fila após criar os links.\nExpiração: {}\n",s["provider"].as_str().unwrap_or_default(),s["storageAccountId"].as_i64().unwrap_or_default(),s["expiresAt"].as_str().unwrap_or("sem expiração")));
        out.push_str(
            "Em caso de falha, tentaremos remover apenas os arquivos criados por esta ação.\n",
        );
    }
    for key in ["files", "attachments"] {
        if let Some(files) = s[key].as_array() {
            for file in files {
                out.push_str(&format!(
                    "Arquivo: {}",
                    visible(file["filename"].as_str().unwrap_or_default())
                ));
                if let Some(size) = file["size"].as_u64() {
                    out.push_str(&format!(
                        " — {:.1} MiB ({size} bytes)",
                        size as f64 / 1048576.0
                    ));
                }
                if let Some(path) = file["sourcePath"].as_str() {
                    out.push_str(&format!("\n  Origem: {}", visible(path)));
                }
                out.push('\n');
            }
        }
    }
    let body = s["body"].as_str().unwrap_or_default();
    let preview = body.chars().take(1600).collect::<String>();
    out.push_str("\nPrévia do corpo (texto; consulte o rascunho para o conteúdo completo):\n");
    out.push_str(&preview);
    if preview.len() < body.len() {
        out.push_str("\n[Prévia abreviada]");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::{
        Platform, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    };
    use slint::{Model, ModelRc, VecModel};
    struct Headless(Rc<MinimalSoftwareWindow>);
    impl Platform for Headless {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }
    }
    #[test]
    fn readable_summary_is_portuguese_and_warns_before_body() {
        let summary = readable_summary(
            &serde_json::json!({"action":"send_cloud_links","accountId":1,"from":{"email":"from@example.invalid"},"to":["to@example.invalid"],"subject":"<not HTML>","anyoneWithLink":true,"provider":"Dropbox","storageAccountId":0,"files":[{"filename":"report.pdf","size":67108864,"sourcePath":"/synthetic/report.pdf"}],"body":"Corpo de teste"}),
        );
        assert!(summary.contains("De: from@example.invalid"));
        assert!(summary.contains("Para: to@example.invalid"));
        assert!(summary.contains("64.0 MiB"));
        assert!(summary.contains("Origem: /synthetic/report.pdf"));
        assert!(summary.contains("qualquer pessoa com o link"));
        assert!(summary.find("qualquer pessoa").unwrap() < summary.find("Corpo de teste").unwrap());
        assert!(!summary.contains("storageAccountId"));
    }
    #[test]
    fn preview_is_bounded_and_untrusted_control_characters_are_visible() {
        let summary = readable_summary(
            &serde_json::json!({"subject":"report\u{202e}\nspoof","body":"a".repeat(5000)}),
        );
        assert!(summary.contains("[Prévia abreviada]"));
        assert!(summary.len() < 2500);
        assert!(summary.contains("\\u{202e}"));
        assert!(summary.contains("\\u{a}"));
    }
    /// Render the actual Slint review UI without mail accounts or credentials.
    /// Run this test alone, since Slint platforms are thread-local.
    #[test]
    #[ignore = "writes a UI screenshot; run explicitly with --exact"]
    fn approval_panel_screenshot() {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(Headless(window.clone()))).unwrap();
        let app = AppWindow::new().unwrap();
        app.set_startup_ready(true);
        app.set_startup_hydrated(true);
        slint::select_bundled_translation("pt_BR").unwrap();
        app.set_ai_pending_count(1);
        app.set_ai_pending_id("synthetic-proposal".into());
        app.set_ai_pending_summary(readable_summary(&serde_json::json!({
            "action":"send_cloud_links", "accountId":1,"from":{"email":"conta@example.invalid"}, "to":["destinatario@example.invalid"],
            "subject":"Relatório para revisão", "body":"Segue o relatório. Exemplo sintético: nenhum upload ou envio real.",
            "anyoneWithLink":true,"provider":"Google Drive","storageAccountId":1,
            "files":[{"filename":"relatorio-grande.pdf", "size":62914560}]
        })).into());
        app.window()
            .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
        app.window().set_size(slint::PhysicalSize::new(1280, 800));
        app.show().unwrap();
        let mut pixels = vec![slint::Rgb8Pixel::default(); 1280 * 800];
        for _ in 0..3 {
            slint::platform::update_timers_and_animations();
            app.window().request_redraw();
            window.draw_if_needed(|renderer| {
                renderer.render(&mut pixels, 1280);
            });
        }
        let bytes: Vec<u8> = pixels
            .iter()
            .flat_map(|pixel| [pixel.r, pixel.g, pixel.b])
            .collect();
        let image = image::RgbImage::from_raw(1280, 800, bytes).unwrap();
        let output = std::env::var_os("MAIL_AI_SCREENSHOT")
            .unwrap_or_else(|| "ai-approval-pt-BR.png".into());
        image.save(output).unwrap();
        assert_eq!(app.get_ai_pending_count(), 1);
        assert!(!app.get_ai_pending_summary().is_empty());
    }
    #[test]
    #[ignore = "writes a synthetic rules screenshot; run explicitly with --exact"]
    fn policy_settings_screenshot() {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(Headless(window.clone()))).unwrap();
        let app = AppWindow::new().unwrap();
        app.set_startup_ready(true);
        app.set_startup_hydrated(true);
        slint::select_bundled_translation("pt_BR").unwrap();
        app.set_ai_policy_open(true);
        app.set_ai_policy_action(4);
        app.set_ai_policy_mode(1);
        app.set_ai_policy_rules(ModelRc::new(VecModel::from(vec![
            "Nova regra".into(),
            "permitir-contatos".into(),
            "bloquear-anexos-externos".into(),
        ])));
        app.set_ai_policy_form(crate::PolicyRuleForm {
            id: "permitir-contatos".into(),
            action_index: 4,
            mode_index: 0,
            accounts: "1".into(),
            domains: "example.invalid".into(),
            known_index: 1,
            max_count: "3".into(),
            cloud_index: 2,
            ..Default::default()
        });
        app.set_ai_policy_history("Exemplo sintético — horário local (America/Cuiaba)\n\n20:00:00  Cliente MCP · Enviar\nRegra: padrão.send · Perguntar\nResultado: aguardando aprovação no aplicativo\nDestinatários: 2; novos: 1; anexos: sim\n\n20:00:15  Aprovação no aplicativo · Enviar\nRegra: padrão.send · Perguntar\nResultado: aprovado; enfileirado (simulado)\n\n20:01:00  Cliente MCP · Upload na nuvem\nRegra: invariante.cloud_requires_app_approval\nResultado: aguardando aprovação no aplicativo\n\n20:02:00  Cliente MCP · Excluir permanentemente\nRegra: padrão.permanent_delete · Bloquear\nResultado: bloqueado\n\nNenhum corpo, assunto, endereço ou token é registrado.".into());
        app.set_ai_policy_status(
            "Dados sintéticos: nenhuma conta real, nenhum upload ou e-mail enviado.".into(),
        );
        app.window()
            .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
        app.window().set_size(slint::PhysicalSize::new(1280, 900));
        app.show().unwrap();
        let mut pixels = vec![slint::Rgb8Pixel::default(); 1280 * 900];
        for _ in 0..3 {
            slint::platform::update_timers_and_animations();
            app.window().request_redraw();
            window.draw_if_needed(|renderer| {
                renderer.render(&mut pixels, 1280);
            });
        }
        let bytes = pixels.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
        let image = image::RgbImage::from_raw(1280, 900, bytes).unwrap();
        image
            .save(
                std::env::var_os("MAIL_AI_POLICY_SCREENSHOT")
                    .unwrap_or_else(|| "ai-policy-settings-pt-BR.png".into()),
            )
            .unwrap();
        assert!(app.get_ai_policy_open());
    }
    #[test]
    #[ignore = "writes a synthetic digest screenshot; run explicitly with --exact"]
    fn daily_digest_screenshot() {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(Headless(window.clone()))).unwrap();
        let app = AppWindow::new().unwrap();
        app.set_startup_ready(true);
        app.set_startup_hydrated(true);
        slint::select_bundled_translation("pt_BR").unwrap();
        app.set_digest_open(true);
        app.set_digest_date("2026-11-06".into());
        app.set_digest_time("08:00".into());
        app.set_digest_account("1".into());
        app.set_digest_summary("6 mensagens locais · America/Cuiaba (UTC-4)\nUrgente: 1 · Cliente: 1 · Financeiro: 1 · Nota fiscal: 1 · Newsletter: 1 · Outros: 1".into());
        app.set_digest_status(
            "Dados sintéticos. Nenhuma conta real, consulta de IA, upload ou e-mail enviado."
                .into(),
        );
        let rows = vec![
            (
                "Urgente",
                "Prazo de revisão termina hoje",
                "cliente@example.invalid",
            ),
            (
                "Cliente",
                "Orçamento para a próxima etapa",
                "compras@example.invalid",
            ),
            (
                "Cobrança e financeiro",
                "Boleto disponível para conferência",
                "financeiro@example.invalid",
            ),
            (
                "Nota fiscal",
                "NF-e recebida: nfe-proc.xml",
                "fiscal@example.invalid",
            ),
            (
                "Propaganda e newsletter",
                "Novidades da semana",
                "news@example.invalid",
            ),
            ("Outros", "Informação geral", "info@example.invalid"),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (category, subject, sender))| crate::TriageRow {
            id: i as i32 + 1,
            sender: sender.into(),
            subject: subject.into(),
            category: category.into(),
            category_index: i as i32,
            unread: true,
        })
        .collect::<Vec<_>>();
        app.set_digest_rows(ModelRc::new(VecModel::from(rows)));
        app.window()
            .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
        app.window().set_size(slint::PhysicalSize::new(1440, 1100));
        app.show().unwrap();
        let mut pixels = vec![slint::Rgb8Pixel::default(); 1440 * 1100];
        for _ in 0..3 {
            slint::platform::update_timers_and_animations();
            app.window().request_redraw();
            window.draw_if_needed(|renderer| {
                renderer.render(&mut pixels, 1440);
            });
        }
        let bytes = pixels.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
        image::RgbImage::from_raw(1440, 1100, bytes)
            .unwrap()
            .save(
                std::env::var_os("MAIL_DIGEST_SCREENSHOT")
                    .unwrap_or_else(|| "daily-digest-pt-BR.png".into()),
            )
            .unwrap();
        assert!(app.get_digest_open());
        assert!(!app.get_digest_email_self());
    }
    #[test]
    #[ignore = "writes synthetic follow-up screenshot; run explicitly"]
    fn followup_screenshot() {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(Headless(window.clone()))).unwrap();
        let app = AppWindow::new().unwrap();
        app.set_startup_ready(true);
        app.set_startup_hydrated(true);
        slint::select_bundled_translation("pt_BR").unwrap();
        app.set_followup_open(true);
        app.set_followup_message("900".into());
        app.set_followup_days("3".into());
        app.set_followup_status("Dados sintéticos. Nenhuma conta real ou envio. Modelos editáveis na janela de composição.".into());
        app.set_followup_rows(ModelRc::new(VecModel::from(vec![
            crate::FollowupRow {
                id: 900,
                subject: "Orçamento · aguardando retorno do cliente".into(),
                due: "06/11/2026 08:00 UTC-4".into(),
                state: "Resposta pendente".into(),
                active: true,
                can_propose: true,
            },
            crate::FollowupRow {
                id: 901,
                subject: "Segunda via de boleto · vencimento confirmado".into(),
                due: "09/11/2026 08:00 UTC-4".into(),
                state: "Aguardando resposta".into(),
                active: true,
                can_propose: false,
            },
            crate::FollowupRow {
                id: 902,
                subject: "Confirmação de pagamento · cliente já respondeu".into(),
                due: "05/11/2026 08:00 UTC-4".into(),
                state: "Respondido".into(),
                active: false,
                can_propose: false,
            },
        ])));
        app.window()
            .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
        app.window().set_size(slint::PhysicalSize::new(1440, 1100));
        app.show().unwrap();
        let mut pixels = vec![slint::Rgb8Pixel::default(); 1440 * 1100];
        for _ in 0..3 {
            slint::platform::update_timers_and_animations();
            app.window().request_redraw();
            window.draw_if_needed(|r| {
                r.render(&mut pixels, 1440);
            });
        }
        image::RgbImage::from_raw(
            1440,
            1100,
            pixels.iter().flat_map(|p| [p.r, p.g, p.b]).collect(),
        )
        .unwrap()
        .save(
            std::env::var_os("MAIL_FOLLOWUP_SCREENSHOT")
                .unwrap_or_else(|| "followups-pt-BR.png".into()),
        )
        .unwrap();
        assert!(app.get_followup_open());
        assert_eq!(app.get_followup_rows().row_count(), 3);
    }
    #[test]
    #[ignore = "writes synthetic financial-document screenshot; run explicitly"]
    fn documents_screenshot() {
        use flectar_mail_core::documents::{DocumentRecord, FinancialDocument, Kind, Party};
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(Headless(window.clone()))).unwrap();
        let app = AppWindow::new().unwrap();
        app.set_startup_ready(true);
        app.set_startup_hydrated(true);
        slint::select_bundled_translation("pt_BR").unwrap();
        app.set_documents_open(true);
        app.set_documents_message("900".into());
        app.set_documents_status(
            "DADOS SINTÉTICOS · Nenhuma conta, pagamento, consulta externa ou envio real.".into(),
        );
        let mut invoice = FinancialDocument::new(Kind::Nfe);
        invoice.issuer = Some(Party {
            cnpj: Some("00000000000000".into()),
            name: Some("EMITENTE SINTÉTICO".into()),
        });
        invoice.recipient = Some(Party {
            cnpj: Some("00000000000000".into()),
            name: Some("DESTINATÁRIO SINTÉTICO".into()),
        });
        invoice.number = Some("1".into());
        invoice.series = Some("1".into());
        invoice.amount_cents = Some(12345);
        invoice.issued_at = Some("2026-11-07T08:00:00-03:00".into());
        invoice.status = Some("100".into());
        invoice.protocol = Some("000000000000000".into());
        invoice.access_key = Some("00000000000000000000000000000000000000000000".into());
        invoice.warn("xml_signature_not_verified");
        invoice.warn("body_amount_mismatch");
        let mut receipt = FinancialDocument::new(Kind::Receipt);
        receipt.amount_cents = Some(12345);
        receipt.issued_at = Some("2026-11-07".into());
        receipt.confidence = "heuristic_medium".into();
        receipt.warn("receipt_is_heuristic_not_payment_confirmation");
        crate::documents_controller::present(
            &app,
            vec![invoice, receipt]
                .into_iter()
                .enumerate()
                .map(|(index, document)| DocumentRecord {
                    message_id: 900,
                    account_id: 1,
                    attachment_id: index as i64 + 1,
                    index: 0,
                    filename: if index == 0 {
                        "nfe-sintetica.xml"
                    } else {
                        "comprovante-sintetico.pdf"
                    }
                    .into(),
                    source_sha256: "synthetic".into(),
                    document,
                })
                .collect(),
        );
        app.window()
            .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
        app.window().set_size(slint::PhysicalSize::new(1440, 1200));
        app.show().unwrap();
        let mut pixels = vec![slint::Rgb8Pixel::default(); 1440 * 1200];
        for _ in 0..3 {
            slint::platform::update_timers_and_animations();
            app.window().request_redraw();
            window.draw_if_needed(|r| {
                r.render(&mut pixels, 1440);
            });
        }
        image::RgbImage::from_raw(
            1440,
            1200,
            pixels.iter().flat_map(|p| [p.r, p.g, p.b]).collect(),
        )
        .unwrap()
        .save(
            std::env::var_os("MAIL_DOCUMENTS_SCREENSHOT")
                .unwrap_or_else(|| "documents-pt-BR.png".into()),
        )
        .unwrap();
        assert!(app.get_documents_open());
        assert_eq!(app.get_documents_rows().row_count(), 2);
    }
}
