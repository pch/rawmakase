//! Apply the native surface color declaration after wgpu's reconfiguration, and
//! read what the window's display can show of HDR.

/// What the window can show of HDR this frame: a view fact, never part of an edit
/// or a render's identity beyond the HDR preview it asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct HdrDisplay {
    /// The display's headroom now, as a linear factor over SDR white (1 = none):
    /// what the HDR preview is encoded for.
    pub(in crate::app) headroom: f32,
    /// The most headroom the display can have; on macOS the current headroom stays
    /// 1 until the window asks for EDR, which it does only while it shows HDR.
    pub(in crate::app) potential: f32,
    /// The window surface holds floats, so values above white reach the display.
    pub(in crate::app) float_surface: bool,
}
impl Default for HdrDisplay {
    fn default() -> Self {
        Self {
            headroom: 1.,
            potential: 1.,
            float_surface: false,
        }
    }
}
impl HdrDisplay {
    /// Whether an HDR photo would look different from its SDR rendition here.
    pub(in crate::app) fn available(&self) -> bool {
        self.float_surface && self.potential > 1.05
    }
    /// The display as the window sees it now.
    pub(in crate::app) fn of(frame: &eframe::Frame) -> Self {
        let float_surface = frame
            .wgpu_render_state()
            .is_some_and(|s| s.target_format == wgpu::TextureFormat::Rgba16Float);
        #[cfg(target_os = "macos")]
        let (headroom, potential) = crate::platform::display::Surface::from_window(frame)
            .map_or((1., 1.), |s| s.headroom());
        #[cfg(not(target_os = "macos"))]
        let (headroom, potential) = (1., 1.);
        Self {
            headroom,
            potential,
            float_surface,
        }
    }
}

#[cfg(target_os = "macos")]
pub(in crate::app) fn paint(
    ui: &eframe::egui::Ui,
    frame: &eframe::Frame,
    device_rgb: bool,
    hdr_shown: bool,
) {
    use crate::platform::display::{Layer, Surface};
    use eframe::{egui, egui_wgpu};

    struct ColorSpace {
        surface: Surface,
        layer: Layer,
    }
    impl egui_wgpu::CallbackTrait for ColorSpace {
        fn paint(
            &self,
            _info: egui::PaintCallbackInfo,
            _pass: &mut wgpu::RenderPass<'static>,
            _resources: &egui_wgpu::CallbackResources,
        ) {
            self.surface.prepare(self.layer);
        }
    }
    let extended = HdrDisplay::of(frame).float_surface;
    if let Some(surface) = Surface::from_window(frame) {
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            ui.clip_rect(),
            ColorSpace {
                surface,
                layer: Layer {
                    device_rgb,
                    extended,
                    edr: hdr_shown,
                },
            },
        ));
    }
}

#[cfg(not(target_os = "macos"))]
pub(in crate::app) fn paint(
    _ui: &eframe::egui::Ui,
    _frame: &eframe::Frame,
    _device_rgb: bool,
    _hdr_shown: bool,
) {
}

#[cfg(test)]
mod tests {
    use super::HdrDisplay;

    /// macOS reports no current headroom until a window asks for EDR, and the window
    /// asks only while it shows HDR: availability has to come from the display's
    /// potential headroom, or HDR would never be shown.
    #[test]
    fn hdr_is_available_from_the_displays_potential_before_edr_is_asked_for() {
        let before_edr = HdrDisplay {
            headroom: 1.,
            potential: 16.,
            float_surface: true,
        };
        assert!(before_edr.available());
        assert!(
            !HdrDisplay {
                float_surface: false,
                ..before_edr
            }
            .available()
        );
        assert!(
            !HdrDisplay {
                potential: 1.,
                ..before_edr
            }
            .available()
        );
    }
}
