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
    /// The display can show HDR, but Windows' HDR switch is off for it.
    pub(in crate::app) hdr_switched_off: bool,
}
impl Default for HdrDisplay {
    fn default() -> Self {
        Self {
            headroom: 1.,
            potential: 1.,
            float_surface: false,
            hdr_switched_off: false,
        }
    }
}
impl HdrDisplay {
    /// Whether an HDR photo would look different from its SDR rendition here.
    pub(in crate::app) fn available(&self) -> bool {
        self.float_surface && self.potential > 1.05
    }
    /// The headroom light above graphics white has on this display now (linear, 1 =
    /// none): what the histogram and the readout mark as shown. None where HDR can't
    /// show at all.
    pub(in crate::app) fn shown_headroom(&self) -> f32 {
        if self.available() {
            self.headroom.max(1.)
        } else {
            1.
        }
    }
    /// Why an HDR photo shows as its SDR rendition here, if it does.
    pub(in crate::app) fn unavailable(&self) -> Option<&'static str> {
        if self.available() {
            None
        } else if self.hdr_switched_off {
            Some("HDR is off in Windows' display settings")
        } else if cfg!(any(target_os = "linux", windows)) && self.potential > 1.05 {
            // The window's surface format was chosen while no display showed HDR.
            Some("HDR shows after RAWmakase restarts")
        } else {
            Some("this display shows no HDR")
        }
    }
    /// The display as the window sees it now.
    pub(in crate::app) fn of(frame: &eframe::Frame, reader: &mut HeadroomReader) -> Self {
        let float_surface = frame
            .wgpu_render_state()
            .is_some_and(|s| extended_surface(s.target_format, s.target_color_space));
        #[cfg(target_os = "macos")]
        let (headroom, potential) = {
            let _ = reader;
            crate::platform::display::Surface::from_window(frame).map_or((1., 1.), |s| s.headroom())
        };
        // A Wayland output's description is what it shows now: turning HDR on or
        // off changes it, so the potential headroom is the current one.
        #[cfg(target_os = "linux")]
        let (headroom, potential) = {
            let anchor = crate::platform::display::wayland::Anchor::of_session();
            let headroom = reader.read(frame).map_or(1., |d| d.headroom(anchor));
            (headroom, headroom)
        };
        // Windows' HDR switch is a display setting, not something a window asks for:
        // the potential headroom is the current one.
        #[cfg(windows)]
        let (headroom, potential, hdr_switched_off) = {
            let hdr = reader.read(frame);
            let headroom = hdr.map_or(1., |h| h.headroom());
            (
                headroom,
                headroom,
                hdr.is_some_and(|h| h.supported && !h.on),
            )
        };
        #[cfg(not(windows))]
        let hdr_switched_off = false;
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        let (headroom, potential) = {
            let _ = reader;
            (1., 1.)
        };
        Self {
            headroom,
            potential,
            float_surface,
            hdr_switched_off,
        }
    }
}

/// Whether a surface of `format` presented in `color_space` shows values above 1:
/// a float surface, which macOS tags extended sRGB itself (`platform::display::Layer`)
/// and other platforms only when it is linear extended sRGB (wgpu's `Auto` may be
/// plain sRGB, which clips).
fn extended_surface(format: wgpu::TextureFormat, color_space: wgpu::SurfaceColorSpace) -> bool {
    format == wgpu::TextureFormat::Rgba16Float
        && (cfg!(target_os = "macos") || color_space == wgpu::SurfaceColorSpace::ExtendedSrgbLinear)
}

/// Reads the window's display's headroom where that needs state kept between frames:
/// on Wayland, the compositor's preferred image description of the window's surface;
/// on Windows, what Windows last said about the window's display.
#[derive(Default)]
pub(in crate::app) struct HeadroomReader {
    /// `None` until the first read; then the window's surface, if it is on a Wayland
    /// compositor with color management.
    #[cfg(target_os = "linux")]
    wayland: Option<Option<crate::platform::display::wayland::Surface>>,
    /// The display Windows was last asked about, when, and what it said.
    #[cfg(windows)]
    windows: Option<(
        crate::platform::display::Monitor,
        std::time::Instant,
        Option<crate::platform::display::WindowsHdr>,
    )>,
}

#[cfg(windows)]
impl HeadroomReader {
    /// What Windows says about the window's display, asked again when the window
    /// reaches another display and every [`RECHECK`]: the HDR switch and the SDR
    /// content brightness change without a message winit passes on. Each read
    /// presents SDR white where Windows shows it (`WindowsHdr::scrgb_white`).
    ///
    /// [`RECHECK`]: crate::app::preferences::monitor::RECHECK
    fn read(&mut self, frame: &eframe::Frame) -> Option<crate::platform::display::WindowsHdr> {
        use crate::platform::display;
        let monitor = display::monitor(frame);
        let now = std::time::Instant::now();
        let hdr = match self.windows {
            Some((m, at, hdr))
                if m == monitor
                    && now.duration_since(at) < crate::app::preferences::monitor::RECHECK =>
            {
                hdr
            }
            _ => {
                let hdr = display::windows_hdr(monitor);
                self.windows = Some((monitor, now, hdr));
                hdr
            }
        };
        // scRGB's 1.0 is 80 cd/m² with HDR on, darker than SDR white.
        if let Some(state) = frame.wgpu_render_state() {
            let output = eframe::egui_wgpu::LinearOutput {
                white: hdr.map_or(1., |h| h.scrgb_white()),
                black: 0.,
            };
            let mut current = state.linear_output.write();
            if *current != output {
                *current = output;
            }
        }
        hdr
    }
}

impl HeadroomReader {
    #[cfg(target_os = "linux")]
    fn read(
        &mut self,
        frame: &eframe::Frame,
    ) -> Option<crate::platform::display::wayland::Description> {
        let description = self
            .wayland
            .get_or_insert_with(|| crate::platform::display::wayland::Surface::from_window(frame))
            .as_mut()?
            .description()?;
        // A linear surface's black is lifted to where the display shows an sRGB
        // window's; SDR white stays at 1 (the compositor anchors it to reference white).
        if let Some(state) = frame.wgpu_render_state() {
            let output = eframe::egui_wgpu::LinearOutput {
                white: 1.,
                black: description.black(crate::platform::display::wayland::Anchor::of_session()),
            };
            let mut current = state.linear_output.write();
            if *current != output {
                *current = output;
            }
        }
        Some(description)
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
    let extended = frame
        .wgpu_render_state()
        .is_some_and(|s| extended_surface(s.target_format, s.target_color_space));
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
    use super::{HdrDisplay, extended_surface};

    /// Off macOS a float surface shows values above 1 only when it is presented as
    /// linear extended sRGB: wgpu's `Auto` may resolve to plain sRGB, which clips.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn only_a_linear_float_surface_reaches_above_sdr_white() {
        use wgpu::{SurfaceColorSpace as Space, TextureFormat as Format};
        assert!(extended_surface(
            Format::Rgba16Float,
            Space::ExtendedSrgbLinear
        ));
        assert!(!extended_surface(Format::Rgba16Float, Space::Auto));
        assert!(!extended_surface(Format::Rgba16Float, Space::Srgb));
        assert!(!extended_surface(
            Format::Bgra8Unorm,
            Space::ExtendedSrgbLinear
        ));
    }

    /// On Linux the surface's format is chosen when the window opens, by whether a
    /// display shows HDR then: a display that shows it later needs a restart, and
    /// says so rather than that it can't.
    #[test]
    #[cfg(target_os = "linux")]
    fn an_hdr_display_on_an_8_bit_surface_asks_for_a_restart() {
        let hdr = HdrDisplay {
            headroom: 4.,
            potential: 4.,
            float_surface: true,
            ..HdrDisplay::default()
        };
        assert_eq!(hdr.unavailable(), None);
        assert_eq!(
            HdrDisplay {
                float_surface: false,
                ..hdr
            }
            .unavailable(),
            Some("HDR shows after RAWmakase restarts")
        );
        assert_eq!(
            HdrDisplay {
                headroom: 1.,
                potential: 1.,
                ..hdr
            }
            .unavailable(),
            Some("this display shows no HDR")
        );
    }

    /// macOS tags the float surface's layer extended sRGB itself.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_macos_float_surface_reaches_above_sdr_white() {
        use wgpu::{SurfaceColorSpace as Space, TextureFormat as Format};
        assert!(extended_surface(Format::Rgba16Float, Space::Auto));
        assert!(!extended_surface(Format::Bgra8Unorm, Space::Auto));
    }

    /// macOS reports no current headroom until a window asks for EDR, and the window
    /// asks only while it shows HDR: availability has to come from the display's
    /// potential headroom, or HDR would never be shown.
    #[test]
    fn hdr_is_available_from_the_displays_potential_before_edr_is_asked_for() {
        let before_edr = HdrDisplay {
            headroom: 1.,
            potential: 16.,
            float_surface: true,
            ..HdrDisplay::default()
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

    /// A display that could show HDR with Windows' HDR switch off says where to turn
    /// it on, whether or not the window's surface could show it yet.
    #[test]
    fn a_display_with_hdr_switched_off_says_so() {
        let off = HdrDisplay {
            hdr_switched_off: true,
            ..HdrDisplay::default()
        };
        assert_eq!(
            off.unavailable(),
            Some("HDR is off in Windows' display settings")
        );
        assert_eq!(
            HdrDisplay {
                float_surface: true,
                ..off
            }
            .unavailable(),
            Some("HDR is off in Windows' display settings")
        );
        assert_eq!(
            HdrDisplay::default().unavailable(),
            Some("this display shows no HDR")
        );
    }
}
