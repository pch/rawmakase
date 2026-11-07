//! Isolated native regression window for #616; no catalog or session is opened.
//! See scripts/test-macos-titlebar.swift. Not part of the headless test suite.
#[cfg(target_os = "macos")]
#[path = "../src/platform/titlebar.rs"]
mod titlebar;

#[cfg(target_os = "macos")]
fn main() -> eframe::Result {
    struct Probe;
    impl eframe::App for Probe {
        fn raw_input_hook(
            &mut self,
            ctx: &eframe::egui::Context,
            raw_input: &mut eframe::egui::RawInput,
        ) {
            titlebar::refresh_window_state(ctx, raw_input);
        }
        fn ui(&mut self, ui: &mut eframe::egui::Ui, frame: &mut eframe::Frame) {
            if std::env::var_os("RAWMAKASE_TITLEBAR_LEGACY").is_none() {
                titlebar::configure(frame, ui.ctx());
            }
            fastframe_titlebar::align_traffic_lights(frame, ui.ctx(), 44.);
            let header = eframe::egui::Rect::from_min_size(
                ui.max_rect().min,
                eframe::egui::vec2(ui.available_width(), 44.),
            );
            if std::env::var_os("RAWMAKASE_TITLEBAR_LEGACY").is_some() {
                fastframe_titlebar::drag(ui, header);
            } else {
                titlebar::drag(ui, header);
            }
            ui.add_space(60.);
            ui.label("RAWmakase title-bar regression probe — no photos or catalog");
            ui.label("Use a real trackpad: double-click at 10 and 40 px below the top edge.");
            ui.label("Check zoom/restore, a held second click, and dragging at both heights.");
            // What the app saves as maximized: it should follow Zoom and its undo.
            let zoomed = ui.input(|input| input.viewport().maximized);
            ui.label(format!("Zoomed, as AppKit reports it: {zoomed:?}"));
            if std::env::var_os("RAWMAKASE_TITLEBAR_MODAL").is_some() {
                eframe::egui::Modal::new(eframe::egui::Id::new("probe-modal")).show(
                    ui.ctx(),
                    |ui| {
                        ui.label("Modal drag regression probe");
                    },
                );
            }
        }
    }
    eframe::run_native(
        "RAWmakase titlebar probe",
        eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_inner_size([700., 400.])
                .with_fullsize_content_view(true)
                .with_titlebar_shown(false)
                .with_title_shown(false),
            ..Default::default()
        },
        Box::new(|_| Ok(Box::new(Probe))),
    )
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("The native title-bar regression probe requires macOS.");
}
