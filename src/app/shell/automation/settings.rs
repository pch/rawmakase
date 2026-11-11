//! Preferences > Automation: whether local scripts and AI clients may reach
//! the editor. MIDI devices have their own page, the MIDI extension's, in
//! Preferences > Extensions; this page points there.
use super::*;
use crate::app::ui::design::{Tone, text};
use crate::app::{
    preferences::{Tab, gap, group},
    ui::theme,
    ui::widgets::form_row,
};

/// The bundled MIDI extension, whose page holds the devices.
const MIDI: &str = "rawmakase.midi";

fn small(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(text::LABEL)
            .color(theme::palette(ui.ctx()).tone(Tone::MUTED)),
    );
}
fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(text::LABEL)
                .color(theme::palette(ui.ctx()).tone(Tone::MUTED)),
        )
        .wrap(),
    );
}

impl Editor {
    pub(in crate::app) fn automation_page(&mut self, ui: &mut egui::Ui) {
        if let Some(error) = &self.controls.load_error {
            hint(ui, error);
            hint(
                ui,
                "Fix the configuration file and restart to edit automation settings. Existing files have been left intact.",
            );
            return;
        }
        let mut settings = self.controls.settings.clone();
        group(ui, "Scripts and AI");
        form_row(ui, "Local access", |ui| {
            ui.checkbox(&mut settings.socket, "Allow local scripts and applications");
        });
        form_row(ui, "Status", |ui| {
            small(
                ui,
                if self.controls.socket.is_some() {
                    "Ready for scripts and MCP clients"
                } else if settings.socket {
                    "Could not start local access"
                } else {
                    "Off"
                },
            );
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                "Connect Codex or another MCP client to control the editor. No MIDI device is required. Access is limited to programs running under your local account.",
            );
        });
        form_row(ui, "Setup", |ui| {
            let error_id = egui::Id::new("mcp-guide-error");
            if ui
                .link("MCP setup · macOS, Linux and Windows")
                .on_hover_text("Open the setup guide in your default browser")
                .clicked()
            {
                // This build omits eframe's links feature. Use the same platform
                // launcher as app updates; build.rs selects matching documentation.
                match crate::platform::web::open(env!("RAWMAKASE_MCP_GUIDE")) {
                    Ok(()) => {
                        ui.data_mut(|d| d.remove_temp::<String>(error_id));
                    }
                    Err(error) => ui.data_mut(|d| {
                        d.insert_temp(error_id, format!("Could not open the setup guide: {error}"));
                    }),
                }
            }
            if let Some(error) = ui.data(|d| d.get_temp::<String>(error_id)) {
                hint(ui, &error);
            }
        });
        gap(ui);
        group(ui, "MIDI devices");
        form_row(ui, "", |ui| {
            hint(
                ui,
                "MIDI control surfaces such as the Loupedeck+ are set up on the MIDI extension's page.",
            );
        });
        form_row(ui, "", |ui| {
            if ui.link("Open MIDI devices in Extensions").clicked() {
                self.extension_page.select(MIDI);
                self.open_preferences(Tab::Extensions);
            }
        });
        if settings != self.controls.settings {
            match settings.save() {
                Ok(()) => self.controls.apply(settings),
                Err(e) => self
                    .messages
                    .fail(format!("Automation settings not saved: {e:#}")),
            }
        }
    }
}
