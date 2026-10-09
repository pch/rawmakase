//! Apply the native surface color declaration after wgpu's reconfiguration.

#[cfg(target_os = "macos")]
pub(super) fn paint(ui: &eframe::egui::Ui, frame: &eframe::Frame, device_rgb: bool) {
    use crate::platform::display::Surface;
    use eframe::{egui, egui_wgpu};

    struct ColorSpace {
        surface: Surface,
        device_rgb: bool,
    }
    impl egui_wgpu::CallbackTrait for ColorSpace {
        fn paint(
            &self,
            _info: egui::PaintCallbackInfo,
            _pass: &mut wgpu::RenderPass<'static>,
            _resources: &egui_wgpu::CallbackResources,
        ) {
            self.surface.prepare(self.device_rgb);
        }
    }
    if let Some(surface) = Surface::from_window(frame) {
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            ui.clip_rect(),
            ColorSpace {
                surface,
                device_rgb,
            },
        ));
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn paint(_ui: &eframe::egui::Ui, _frame: &eframe::Frame, _device_rgb: bool) {}
