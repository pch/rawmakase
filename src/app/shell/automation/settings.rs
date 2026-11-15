//! Local access: whether local scripts and MCP clients may reach the editor
//! through the control socket. Shown twice, as one setting: in Preferences >
//! General for scripts, and on the MCP extension's page for its clients,
//! which reach the editor through the same socket.
use super::*;
use crate::app::ui::design::{Tone, text};
use crate::app::{ui::theme, ui::widgets::form_row};

/// Whether the control socket admits local programs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) enum LocalAccess {
    Off,
    /// On, and listening.
    Ready,
    /// On, but the socket couldn't start.
    Failed,
    /// `automation.json` couldn't be read, so it stays as it was.
    Unreadable,
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
    /// Whether local scripts and MCP clients can reach the editor now.
    pub(in crate::app) fn local_access(&self) -> LocalAccess {
        if self.controls.load_error.is_some() {
            LocalAccess::Unreadable
        } else if self.controls.socket.is_some() {
            LocalAccess::Ready
        } else if self.controls.settings.socket {
            LocalAccess::Failed
        } else {
            LocalAccess::Off
        }
    }

    /// Local access's switch and status, saved to `automation.json` as it is
    /// changed. `ready` says what it is ready for where it is shown.
    pub(in crate::app) fn local_access_rows(&mut self, ui: &mut egui::Ui, ready: &str) {
        if let Some(error) = &self.controls.load_error {
            form_row(ui, "Local access", |ui| hint(ui, error));
            form_row(ui, "", |ui| {
                hint(
                    ui,
                    "Fix the configuration file and restart to change local access. Existing files have been left intact.",
                );
            });
            return;
        }
        let mut settings = self.controls.settings.clone();
        form_row(ui, "Local access", |ui| {
            ui.checkbox(&mut settings.socket, "Allow local scripts and applications");
        });
        let status = match self.local_access() {
            LocalAccess::Ready => ready,
            LocalAccess::Failed => "Could not start local access",
            LocalAccess::Off | LocalAccess::Unreadable => "Off",
        };
        form_row(ui, "Status", |ui| hint(ui, status));
        form_row(ui, "", |ui| {
            hint(
                ui,
                "One setting for scripts (rawmakase control) and MCP clients alike. Only programs running under your account can connect.",
            );
        });
        if settings != self.controls.settings {
            match settings.save() {
                Ok(()) => self.controls.apply(settings),
                Err(e) => self.messages.fail(format!("Local access not saved: {e:#}")),
            }
        }
    }

    /// The MCP setup guide, opened in the browser.
    pub(in crate::app) fn mcp_guide_row(&mut self, ui: &mut egui::Ui) {
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
    }
}
