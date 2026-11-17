//! Cancel buttons for long jobs: each job's token is kept in egui's
//! memory under the window's key, installed on the worker thread, and
//! cancelled from the button next to the spinner.

use eframe::egui;
use trace_core::cancel::Token;

fn id(key: &str) -> egui::Id {
    egui::Id::new(("job-cancel", key))
}

/// A fresh token for a job about to start in window `key`.
pub fn start(ctx: &egui::Context, key: &str) -> Token {
    let t = Token::new();
    ctx.data_mut(|d| d.insert_temp(id(key), t.clone()));
    t
}

/// The cancel button beside a running job; true when it was pressed
/// (the job is asked to stop).
pub fn cancel_button(ui: &mut egui::Ui, key: &str) -> bool {
    let pressed = ui
        .button("✖ Cancel")
        .on_hover_text("Stop this job")
        .clicked();
    if pressed {
        if let Some(t) = ui.ctx().data(|d| d.get_temp::<Token>(id(key))) {
            t.cancel();
        }
    }
    pressed
}
