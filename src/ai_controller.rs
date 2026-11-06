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
        "reply" => "Responder email",
        "reply_all" => "Responder a todos",
        "move_to_trash" => "Mover para a lixeira",
        "delete_permanently" => "Excluir permanentemente",
        _ => "Revisar ação",
    };
    let mut out = format!("{action}\n");
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
}
