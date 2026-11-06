//! UI-only approval entry point for the embedded MCP tool layer.
use crate::{AppWindow, InboxState};
use slint::ComponentHandle;
use std::{cell::RefCell, rc::Rc, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Duration};
use tokio::runtime::Handle;

pub(crate) fn register(app: &AppWindow, state: &Rc<RefCell<InboxState>>, runtime: &Handle) -> slint::Timer {
    let weak_state = Rc::downgrade(state);
    let weak_app = app.as_weak();
    let handle = runtime.clone();
    app.on_ai_review_action(move |id, approve| {
        let Some(state) = weak_state.upgrade() else { return; };
        let Some(source) = state.borrow().core.clone() else { return; };
        let server = source.ai_server();
        let weak_app = weak_app.clone();
        if let Some(app) = weak_app.upgrade() {
            if app.get_ai_review_busy() { return; }
            app.set_ai_review_busy(true);
            app.set_ai_review_status("".into());
        }
        handle.spawn(async move {
            // Keep the profile lease and listener alive for the whole operation.
            let _source = source;
            let result = if approve { server.approve_in_app(&id).await } else { server.cancel_in_app(&id).await };
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
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(750), move || {
        let Some(state) = weak_state.upgrade() else { return; };
        let Some(source) = state.borrow().core.clone() else { return; };
        let Some(app) = weak_app.upgrade() else { return; };
        // Freeze the visible proposal while a decision is being processed.
        if app.get_ai_review_busy() || in_flight.swap(true, Ordering::AcqRel) { return; }
        let weak_app = weak_app.clone();
        let in_flight = in_flight.clone();
        handle.spawn(async move {
            let actions = source.ai_server().pending_actions().await;
            let _ = weak_app.upgrade_in_event_loop(move |app| {
                if !app.get_ai_review_busy() {
                    app.set_ai_pending_count(actions.len() as i32);
                    if let Some(first) = actions.first() {
                        app.set_ai_pending_id(first["pendingId"].as_str().unwrap_or_default().into());
                        app.set_ai_pending_summary(serde_json::to_string_pretty(&first["summary"]).unwrap_or_default().into());
                    } else {
                        app.set_ai_pending_id("".into());
                        app.set_ai_pending_summary("".into());
                    }
                }
                in_flight.store(false, Ordering::Release);
            });
        });
    });
    timer
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::{Platform, WindowAdapter, WindowEvent, software_renderer::{MinimalSoftwareWindow, RepaintBufferType}};
    struct Headless(Rc<MinimalSoftwareWindow>);
    impl Platform for Headless {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> { Ok(self.0.clone()) }
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
        app.set_ai_pending_summary(serde_json::to_string_pretty(&serde_json::json!({
            "action":"send", "from":"conta@example.invalid", "to":["destinatario@example.invalid"],
            "subject":"Relatório para revisão", "body":"Segue o relatório. Este é um exemplo sem envio.",
            "attachments":[{"filename":"relatorio.pdf", "mimeType":"application/pdf"}]
        })).unwrap().into());
        app.window().dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
        app.window().set_size(slint::PhysicalSize::new(1280, 800));
        app.show().unwrap();
        let mut pixels = vec![slint::Rgb8Pixel::default(); 1280 * 800];
        for _ in 0..3 {
            slint::platform::update_timers_and_animations();
            app.window().request_redraw();
            window.draw_if_needed(|renderer| { renderer.render(&mut pixels, 1280); });
        }
        let bytes: Vec<u8> = pixels.iter().flat_map(|pixel| [pixel.r, pixel.g, pixel.b]).collect();
        let image = image::RgbImage::from_raw(1280, 800, bytes).unwrap();
        let output = std::env::var_os("MAIL_AI_SCREENSHOT").unwrap_or_else(|| "ai-approval-pt-BR.png".into());
        image.save(output).unwrap();
        assert_eq!(app.get_ai_pending_count(), 1);
        assert!(!app.get_ai_pending_summary().is_empty());
    }
}
