//! Example domain-specific UI; calculations and configuration authority stay in C++.
use crate::{OverlayActions, OverlayState};
use liboverlay::egui::egui;

pub fn show(ctx: &egui::Context, state: OverlayState) -> OverlayActions {
    let mut enabled = state.alerts_enabled != 0;
    let mut threshold = state.alert_threshold;
    let mut changed = false;
    let mut close = false;
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.heading("CPU monitor");
        ui.label(format!("Average CPU: {:.1}%", state.cpu_percent));
        ui.add(egui::ProgressBar::new(state.cpu_percent / 100.0).show_percentage());
        if state.alert_active != 0 {
            ui.colored_label(egui::Color32::RED, "High CPU usage");
        }
        changed |= ui.checkbox(&mut enabled, "Enable alerts").changed();
        changed |= ui
            .add(egui::Slider::new(&mut threshold, 0.0..=100.0).text("Threshold (%)"))
            .changed();
        close = ui.button("Close overlay").clicked();
    });
    OverlayActions {
        settings_changed: u32::from(changed),
        alert_threshold: threshold,
        alerts_enabled: u32::from(enabled),
        close_requested: u32::from(close),
    }
}
