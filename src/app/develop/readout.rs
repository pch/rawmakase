//! Lightroom's RGB readout under the histogram: while the pointer is over the photo
//! being edited, the R, G and B percentages of the processed pixel under it take
//! the place of the exposure line.
//!
//! The values come from the rendered photo (encoded sRGB, before clipping warnings
//! and the monitor profile), not from the screen. At Fit that is one pixel of the
//! preview, which averages the photo's pixels beneath it; at 100% and above, one
//! pixel of the photo. They are shown in Lightroom's Melissa RGB: ProPhoto
//! primaries with the sRGB tone curve. Renders keep their pixels for it only while
//! the pointer is over the photo, so dragging sliders costs nothing extra.
//!
//! An HDR edit whose master renders reads the master instead (docs/hdr.md#histogram):
//! the HDR preview's where it shows, otherwise the HDR histogram's, so an SDR display
//! reads it too. A channel up to graphics white is its Melissa RGB percentage; one
//! above it, its stops above white (`+1.3`), as Lightroom writes them: orange where
//! the display shows it, red beyond the display's headroom or at the HDR limit.
use crate::app::Editor;
use crate::camera_profiles::RGB_TO_PRO;
use crate::color::{mul, srgb_decode, srgb_encode};
use eframe::egui::{Pos2, Rect};

/// The readout's state: whether the pointer is over the photo, and the values
/// under it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::app) struct Readout {
    /// The pointer is over the photo being edited, so renders keep their pixels.
    pub(in crate::app) hovering: bool,
    /// The values under the pointer, once the pixels are there.
    pub(in crate::app) values: Option<Values>,
}
/// What the readout read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) enum Values {
    /// Melissa RGB percentages of the SDR rendition.
    Sdr([f32; 3]),
    /// The HDR master's channels (linear ProPhoto, 1 = graphics white), with the
    /// display's headroom now (linear, 1 = none) and the HDR limit's peak.
    Hdr {
        master: [f32; 3],
        headroom: f32,
        peak: f32,
    },
}
/// How a part of the readout is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Shade {
    /// An SDR value, or a label.
    Plain,
    /// Light above white the display shows.
    Within,
    /// Light above white beyond the display's headroom or at the HDR limit.
    Beyond,
}
impl Readout {
    /// The line shown under the histogram, as Lightroom's.
    #[cfg(test)]
    pub(in crate::app) fn text(&self) -> Option<String> {
        self.segments()
            .map(|s| s.into_iter().map(|(t, _)| t).collect())
    }
    /// The line shown under the histogram, as Lightroom's, in parts, each with its
    /// colour.
    pub(in crate::app) fn segments(&self) -> Option<Vec<(String, Shade)>> {
        Some(match self.values? {
            Values::Sdr(percent) => vec![(text(percent), Shade::Plain)],
            Values::Hdr {
                master,
                headroom,
                peak,
            } => hdr_segments(master, headroom, peak),
        })
    }
}
/// The HDR readout's parts: each channel's percentage up to graphics white, or its
/// stops above it, shaded by whether the display shows it.
fn hdr_segments(master: [f32; 3], headroom: f32, peak: f32) -> Vec<(String, Shade)> {
    let mut parts = Vec::new();
    for (i, (name, v)) in ["R", "G", "B"].into_iter().zip(master).enumerate() {
        let gap = if i == 0 { "" } else { "   " };
        if v <= 1. {
            let percent = melissa_channel(v);
            parts.push((format!("{gap}{name} {percent:.1}%"), Shade::Plain));
        } else {
            parts.push((format!("{gap}{name} "), Shade::Plain));
            let beyond = v > headroom || v >= crate::rendered::HIGHLIGHT_CLIP * peak;
            let shade = if beyond { Shade::Beyond } else { Shade::Within };
            parts.push((format!("+{:.1}", v.log2()), shade));
        }
    }
    parts
}
/// A linear ProPhoto channel up to graphics white as its Melissa RGB percentage.
fn melissa_channel(v: f32) -> f32 {
    srgb_encode(v.clamp(0., 1.)) * 100.
}
/// Melissa RGB percentages as Lightroom writes them.
pub(in crate::app) fn text([r, g, b]: [f32; 3]) -> String {
    format!("R {r:.1}   G {g:.1}   B {b:.1} %")
}

/// An encoded sRGB colour (0–1) as Melissa RGB percentages: ProPhoto primaries
/// (adapted to D50) with the sRGB tone curve, Lightroom's readout space.
pub(in crate::app) fn melissa_percent(srgb: [f32; 3]) -> [f32; 3] {
    let linear = srgb.map(|v| srgb_decode(v.clamp(0., 1.)));
    mul(RGB_TO_PRO, linear).map(|v| srgb_encode(v.clamp(0., 1.)) * 100.)
}

/// The pixel of `samples` under `pos`, where they are drawn at `shown`; None
/// off them.
pub(in crate::app) fn pixel_at(
    samples: &image::RgbImage,
    shown: Rect,
    pos: Pos2,
) -> Option<[f32; 3]> {
    let (w, h) = samples.dimensions();
    let (x, y) = index_at(w, h, shown, pos)?;
    Some(samples.get_pixel(x, y).0.map(|v| f32::from(v) / 255.))
}

/// The column and row of a `w` × `h` image drawn at `shown` under `pos`; None off
/// it.
pub(in crate::app) fn index_at(w: u32, h: u32, shown: Rect, pos: Pos2) -> Option<(u32, u32)> {
    if !shown.contains(pos) || w == 0 || h == 0 {
        return None;
    }
    let at = |p: f32, lo: f32, len: f32, n: u32| {
        (((p - lo) / len * n as f32).floor().max(0.) as u32).min(n.saturating_sub(1))
    };
    Some((
        at(pos.x, shown.left(), shown.width(), w),
        at(pos.y, shown.top(), shown.height(), h),
    ))
}

impl Editor {
    /// Renders keep the shown pixels: for an eyedropper's loupe, or the readout.
    pub(in crate::app) fn wants_samples(&self) -> bool {
        self.view.picks_color() || self.view.readout.hovering
    }
    /// The processed colour at `pos` on the photo drawn at `rect`, with the 100%
    /// region over it at `region`: from the region where it covers `pos`.
    pub(in crate::app) fn shown_pixel(
        &self,
        pos: Pos2,
        rect: Rect,
        region: Option<Rect>,
    ) -> Option<[f32; 3]> {
        if let (Some(samples), Some(region)) = (&self.preview.region_samples, region)
            && let Some(pixel) = pixel_at(samples, region, pos)
        {
            return Some(pixel);
        }
        pixel_at(self.preview.samples.as_ref()?, rect, pos)
    }
    /// The HDR master's values at `pos`, where the edit's master renders: from the
    /// HDR preview that shows there (its region, then the whole photo), else from
    /// the HDR histogram's master of the edit as it is now.
    fn master_pixel(&self, pos: Pos2, rect: Rect) -> Option<Values> {
        let master = |m: [f32; 3], peak: f32| Values::Hdr {
            master: m,
            headroom: self.view.hdr_display.shown_headroom(),
            peak,
        };
        let peak = self.effective_recipe().hdr.peak();
        for (slot, key, at) in self.hdr_preview_reads(rect) {
            if let Some(m) = self.hdr_preview.master_at(slot, &key, at, pos) {
                return Some(master(m, peak));
            }
        }
        let measured = self.hdr_histogram_master()?;
        let (x, y) = index_at(measured.width, measured.height, rect, pos)?;
        let m = *measured.pixels.get((y * measured.width + x) as usize)?;
        Some(master(m, measured.peak))
    }
    /// The HDR previews of the edit the readout reads, in order, each with where it
    /// draws over the photo drawn at `rect`: its 100% region, where its own key
    /// places it, then the whole photo under it, which draws only with the region;
    /// or the whole photo.
    pub(in crate::app) fn hdr_preview_reads(
        &self,
        rect: Rect,
    ) -> Vec<(
        crate::app::develop::hdr_preview::Slot,
        crate::app::develop::hdr_preview::Key,
        Rect,
    )> {
        use crate::app::develop::viewport::hdr_drawn_at;
        let (Ok(due), Some(image)) = (self.hdr_preview_due(), self.document.full()) else {
            return Vec::new();
        };
        let g = crate::develop::Geometry::new(image, &self.effective_recipe(), 0);
        let (slot, other) = due.slots(false);
        let mut reads = vec![(
            slot,
            due.key.clone(),
            hdr_drawn_at(rect, &g, due.key.part()),
        )];
        if let Some(under) = due.under
            && self.hdr_preview.shows(slot, &due.key)
        {
            reads.push((other, under, rect));
        }
        reads
    }
    /// Follows the pointer over the photo being edited (`hover`, on the photo drawn
    /// at `rect`): renders keep their pixels while it is there (the viewport asks for
    /// one when they start to), and reads the values under it.
    pub(in crate::app) fn update_readout(
        &mut self,
        hover: Option<Pos2>,
        rect: Rect,
        region: Option<Rect>,
    ) {
        let hover = hover.filter(|p| rect.contains(*p));
        self.view.readout.hovering = hover.is_some();
        let values = hover.and_then(|pos| {
            self.master_pixel(pos, rect).or_else(|| {
                self.shown_pixel(pos, rect, region)
                    .map(melissa_percent)
                    .map(Values::Sdr)
            })
        });
        if values != self.view.readout.values {
            self.view.readout.values = values;
            // The histogram is drawn before the photo: show the new values now.
            self.context.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Vec2;

    #[test]
    fn values_are_melissa_rgb_percentages() {
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.1);
        assert!(close(melissa_percent([1.; 3]), [100.; 3]));
        assert!(close(melissa_percent([0.; 3]), [0.; 3]));
        // Neutrals keep their sRGB tone curve value.
        assert!(close(melissa_percent([0.5; 3]), [50.; 3]));
        // sRGB's red sits well inside ProPhoto's gamut.
        let red = melissa_percent([1., 0., 0.]);
        assert!(close(red, [75.4, 34.6, 13.8]), "{red:?}");
        assert_eq!(text([75.43, 34.6, 13.8]), "R 75.4   G 34.6   B 13.8 %");
    }

    /// An HDR channel up to white reads as its percentage, one above as its stops
    /// above white: orange where the display shows it, red beyond its headroom or at
    /// the HDR limit.
    #[test]
    fn hdr_values_read_in_stops_above_white() {
        let readout = |master, headroom| Readout {
            hovering: true,
            values: Some(Values::Hdr {
                master,
                headroom,
                peak: 16.,
            }),
        };
        let r = readout([0.18, 2.5, 15.99], 4.);
        assert_eq!(r.text().as_deref(), Some("R 46.1%   G +1.3   B +4.0"));
        let shades: Vec<Shade> = r.segments().unwrap().into_iter().map(|(_, s)| s).collect();
        assert_eq!(
            shades,
            [
                Shade::Plain,
                Shade::Plain,
                Shade::Within,
                Shade::Plain,
                Shade::Beyond
            ]
        );
        // On a display without headroom, all light above white is beyond it.
        let sdr = readout([1., 2.5, 0.], 1.);
        assert_eq!(sdr.text().as_deref(), Some("R 100.0%   G +1.3   B 0.0%"));
        assert!(
            sdr.segments()
                .unwrap()
                .contains(&("+1.3".into(), Shade::Beyond))
        );
    }

    #[test]
    fn the_pixel_under_the_pointer_follows_zoom_pan_and_the_region() {
        // A 4 × 2 preview, each pixel its own value.
        let samples = image::RgbImage::from_fn(4, 2, |x, y| {
            image::Rgb([(x * 50) as u8, (y * 100) as u8, 0])
        });
        let shown = Rect::from_min_size(Pos2::new(100., 50.), Vec2::new(400., 200.));
        let value = |pos: Pos2| pixel_at(&samples, shown, pos).map(|p| (p[0] * 255.).round() as u8);
        assert_eq!(value(Pos2::new(100., 50.)), Some(0));
        assert_eq!(value(Pos2::new(399., 60.)), Some(100));
        assert_eq!(value(Pos2::new(400., 60.)), Some(150));
        assert_eq!(value(Pos2::new(500., 250.)), Some(150));
        assert_eq!(value(Pos2::new(99., 60.)), None);
        // Panned half off the left of the screen, the photo's own pixels still line up.
        let panned = shown.translate(Vec2::new(-300., 0.));
        assert_eq!(
            pixel_at(&samples, panned, Pos2::new(110., 60.)).map(|p| (p[0] * 255.).round() as u8),
            Some(150)
        );
    }

    #[test]
    fn hovering_the_photo_asks_for_its_pixels_and_reads_the_region_first() {
        let ctx = eframe::egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(200., 100.));
        assert!(!e.wants_samples());
        e.update_readout(Some(Pos2::new(50., 50.)), rect, None);
        assert!(e.wants_samples());
        // No pixels yet: nothing to read.
        assert_eq!(e.view.readout.text(), None);
        e.preview.samples = Some(image::RgbImage::from_pixel(
            2,
            1,
            image::Rgb([255, 255, 255]),
        ));
        // The 100% region, drawn over part of the photo, is read where it is.
        let region = Rect::from_min_size(Pos2::new(40., 40.), Vec2::new(20., 20.));
        e.preview.region_samples = Some(image::RgbImage::from_pixel(1, 1, image::Rgb([0, 0, 0])));
        e.update_readout(Some(Pos2::new(50., 50.)), rect, Some(region));
        assert_eq!(e.view.readout.values, Some(Values::Sdr([0.; 3])));
        e.update_readout(Some(Pos2::new(150., 50.)), rect, Some(region));
        assert_eq!(
            e.view.readout.text().as_deref(),
            Some("R 100.0   G 100.0   B 100.0 %")
        );
        // Another photo: no values until its pixels arrive.
        e.view.clear_document();
        e.preview.clear_document();
        assert!(e.preview.samples.is_none() && e.preview.region_samples.is_none());
        assert_eq!(e.view.readout.text(), None);
        // Off the photo: the readout goes, and so do the renders' pixels.
        e.update_readout(Some(Pos2::new(250., 50.)), rect, None);
        assert_eq!(e.view.readout, Readout::default());
        assert!(!e.wants_samples());
    }

    #[test]
    fn pointing_back_at_an_unchanged_photo_renders_nothing() {
        let ctx = eframe::egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let pixel = || image::RgbImage::from_pixel(1, 1, image::Rgb([255, 255, 255]));
        // The first hover asks a render for the pixels.
        assert!(e.preview.ask_for_samples());
        assert!(!e.preview.ask_for_samples());
        e.preview.keep_samples(false, Some(pixel()));
        e.preview.keep_samples(true, Some(pixel()));
        // Off the photo and back, with nothing rendered since: the pixels kept still
        // show it.
        e.preview.stop_asking_for_samples();
        assert!(e.preview.samples.is_some() && e.preview.region_samples.is_some());
        assert!(!e.preview.ask_for_samples());
        // A render on its way when the pointer returns has no pixels: ask again.
        e.preview.stop_asking_for_samples();
        let (render, _) = e.preview.task.start();
        assert!(e.preview.ask_for_samples());
        e.preview.task.finish(render);
        // One that landed while away drops them all, whole photo and region.
        e.preview.stop_asking_for_samples();
        e.preview.keep_samples(false, None);
        assert!(e.preview.samples.is_none() && e.preview.region_samples.is_none());
        assert!(e.preview.ask_for_samples());
        // Zoomed to 100%, where renders keep only the region's pixels: those are
        // the ones shown.
        e.preview.mode = crate::app::develop::state::TextureMode::Region([0, 0, 1, 1]);
        e.preview.keep_samples(true, Some(pixel()));
        e.preview.stop_asking_for_samples();
        assert!(!e.preview.ask_for_samples());
        // Back to Fit, the whole photo's pixels are needed.
        e.preview.stop_asking_for_samples();
        e.preview.mode = crate::app::develop::state::TextureMode::Whole;
        assert!(e.preview.ask_for_samples());
    }

    #[test]
    fn i_cycles_the_info_overlay_in_develop() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let photos = dir.path().join("photos");
        std::fs::create_dir(&photos)?;
        std::fs::write(photos.join("image.ARW"), b"identity fixture")?;
        let catalog = dir.path().join("test.rawmakase");
        crate::catalog::Catalog::create(&catalog)?.add_folder(&photos)?;
        let ctx = eframe::egui::Context::default();
        let library = crate::app::library::Library::load(&catalog, ctx.clone())?;
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        e.library = Some(Box::new(library));
        let none = eframe::egui::Modifiers::NONE;
        // Each frame's I events: pressed, released or both, with the modifiers held
        // for them.
        let mut frame = |events: &[bool], modifiers: eframe::egui::Modifiers| {
            let events = events
                .iter()
                .map(|&pressed| eframe::egui::Event::Key {
                    key: eframe::egui::Key::I,
                    physical_key: Some(eframe::egui::Key::I),
                    pressed,
                    repeat: false,
                    modifiers,
                })
                .collect();
            let input = eframe::egui::RawInput {
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |_| e.develop_shortcuts(&ctx));
            output.textures_delta.clear();
            e.library.as_ref().unwrap().loupe_info()
        };
        assert_eq!(frame(&[true, false], none), "Info1");
        assert_eq!(frame(&[true], none), "Info2");
        // Held down, it repeats without flickering through them.
        assert_eq!(frame(&[true], none), "Info2");
        assert_eq!(frame(&[false], none), "Info2");
        assert_eq!(frame(&[true, false], none), "Off");
        // Cmd+I is not I, even when Cmd is let go in the same frame.
        let cmd = eframe::egui::Modifiers::COMMAND;
        assert_eq!(frame(&[true, false], cmd), "Off");
        Ok(())
    }
}
