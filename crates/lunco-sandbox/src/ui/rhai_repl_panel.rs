//! In-app **rhai REPL** panel — submit snippets to the running app and see their
//! stdout, on web *and* native.
//!
//! It runs rhai through the exact same path as `window.lunco_rhai(...)`, the
//! native `sandbox rhai` CLI client, and MCP: a `RunRhai` command over the
//! in-process API bridge ([`lunco_api::ApiBridge`]). No sockets, no CLI — so it
//! works in the browser where the TCP-client `rhai_repl` module cannot.
//!
//! Gated on the bridge's availability (`transport-http` on native, always on
//! wasm) — see the `transport-http` feature in `Cargo.toml`.
#![cfg(any(target_arch = "wasm32", feature = "transport-http"))]

use std::sync::{Arc, Mutex};

use bevy_egui::egui;
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};

/// Shared inbox: async bridge tasks push `(code, output)` here; `render` drains
/// it into `history`. `Arc<Mutex<…>>` (not a channel) keeps the panel
/// `Send + Sync` without pulling a channel crate.
type Inbox = Arc<Mutex<Vec<(String, String)>>>;

pub(crate) struct RhaiReplPanel {
    input: String,
    /// Newest last: `(submitted code, response text)`.
    history: Vec<(String, String)>,
    inbox: Inbox,
}

impl Default for RhaiReplPanel {
    fn default() -> Self {
        Self {
            input: String::new(),
            history: Vec::new(),
            inbox: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Panel for RhaiReplPanel {
    fn id(&self) -> PanelId {
        PanelId("rhai_repl")
    }
    fn title(&self) -> String {
        "🐚 Rhai".into()
    }
    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Tools
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Bottom
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        // Drain any completed submissions into the visible history.
        if let Ok(mut pending) = self.inbox.lock() {
            self.history.append(&mut pending);
        }

        ui.horizontal(|ui| {
            ui.heading("Rhai REPL");
            if ui.button("Clear").clicked() {
                self.history.clear();
            }
        });
        ui.label(
            egui::RichText::new("Runs against the live app — same RunRhai path as the API/MCP.")
                .weak()
                .small(),
        );
        ui.separator();

        egui::ScrollArea::vertical()
            .max_height(200.0)
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.history.is_empty() {
                    ui.label(egui::RichText::new("No output yet. Try: pause()").weak());
                }
                for (code, out) in &self.history {
                    ui.label(egui::RichText::new(format!("› {code}")).monospace().strong());
                    ui.label(egui::RichText::new(out).monospace().weak());
                    ui.add_space(4.0);
                }
            });

        ui.separator();

        // Ctrl/Cmd+Enter or the Run button submits; a bare Enter inserts a newline
        // (multiline editor) so multi-statement snippets are easy.
        let editor = ui.add(
            egui::TextEdit::multiline(&mut self.input)
                .code_editor()
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .hint_text("restart_scene(); pause()"),
        );
        let key_submit = editor.has_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
        let btn_submit = ui.button("Run ▶  (Ctrl/Cmd+Enter)").clicked();

        if (key_submit || btn_submit) && !self.input.trim().is_empty() {
            let code = std::mem::take(&mut self.input);
            match ctx.resource::<lunco_api::ApiBridge>().cloned() {
                Some(bridge) => spawn_rhai(bridge, code, self.inbox.clone()),
                None => {
                    // Bridge not installed (a build without the API transport).
                    self.history
                        .push((code, "(API bridge unavailable in this build)".into()));
                }
            }
            editor.request_focus();
        }
    }
}

/// Submit `code` as a `RunRhai` command through the bridge on a detached task,
/// pushing `(code, output)` into `inbox` when the ECS produces the response.
fn spawn_rhai(bridge: lunco_api::ApiBridge, code: String, inbox: Inbox) {
    let req = match lunco_api::rhai_request(&code) {
        Ok(r) => r,
        Err(e) => {
            if let Ok(mut v) = inbox.lock() {
                v.push((code, format!("request error: {e}")));
            }
            return;
        }
    };
    let fut = async move {
        let out = match bridge.0.execute(req).await {
            Ok(resp) => format_response(resp),
            Err(()) => "request dropped (app shutting down?)".to_string(),
        };
        if let Ok(mut v) = inbox.lock() {
            v.push((code, out));
        }
    };
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(fut);
    #[cfg(not(target_arch = "wasm32"))]
    bevy::tasks::AsyncComputeTaskPool::get().spawn(fut).detach();
}

/// Render an [`ApiResponse`](lunco_api::schema::ApiResponse) as REPL output via
/// the same envelope the HTTP/JS transports use (forward-compatible with new
/// variants). A bare string `data` (the common rhai stdout shape) is shown as-is.
fn format_response(resp: lunco_api::schema::ApiResponse) -> String {
    let env = lunco_api::transports::ApiResponseEnvelope::from(resp);
    if let Some(err) = env.error {
        return format!("error: {err}");
    }
    match env.data {
        Some(serde_json::Value::String(s)) => s,
        Some(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string()),
        None => "ok".to_string(),
    }
}
