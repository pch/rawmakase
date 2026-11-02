//! The camera thumbnail view in Develop: the JPEG the camera embedded in the RAW, in
//! place of the photo and labelled as such, toggled as Before alone is (Shift+\ or the
//! Photo menu). It shows what Camera Look
//! (fitted) and the Library's first preview start from: unedited, uncropped, fitted
//! to the window. Read once per photo, off the UI thread, when first asked for.
use super::{Editor, before_after::Compare, worker::Event};
use crate::app::design::{Tone, text};
use crate::app::theme;
use eframe::egui::{self, Color32, Pos2, Rect, Vec2};
use std::path::{Path, PathBuf};

/// The longest edge kept: some cameras embed a 60 MP JPEG.
const MAX_EDGE: u32 = 4096;

/// The view's label over the photo, without a named look.
const LABEL: &str = "Camera thumbnail: embedded JPEG, no edits";

/// A read thumbnail. It keeps the sRGB image: the texture follows the monitor's
/// profile, which can change while the view is open.
pub(super) struct Thumbnail {
    srgb: image::RgbImage,
    look: Option<String>,
    monitor: Option<PathBuf>,
    texture: egui::TextureHandle,
}
impl Thumbnail {
    fn new(
        ctx: &egui::Context,
        srgb: image::RgbImage,
        look: Option<String>,
        monitor: Option<PathBuf>,
    ) -> Self {
        let texture = upload(ctx, &srgb, monitor.as_deref());
        Self {
            srgb,
            look,
            monitor,
            texture,
        }
    }
    /// Converts again when the monitor's profile is not the one it was shown for.
    fn follow(&mut self, ctx: &egui::Context, monitor: Option<PathBuf>) {
        if self.monitor != monitor {
            self.texture = upload(ctx, &self.srgb, monitor.as_deref());
            self.monitor = monitor;
        }
    }
    /// The view's label over the photo.
    fn label(&self) -> String {
        match &self.look {
            Some(look) => format!("Camera thumbnail: {look}, embedded JPEG, no edits"),
            None => LABEL.to_owned(),
        }
    }
}

fn upload(
    ctx: &egui::Context,
    srgb: &image::RgbImage,
    monitor: Option<&Path>,
) -> egui::TextureHandle {
    let mut rgb = srgb.as_raw().clone();
    if let Some(p) = monitor {
        // Without the profile it is still the right picture, as sRGB.
        let _ = crate::color_management::display_transform(p, &mut rgb);
    }
    let size = [srgb.width() as usize, srgb.height() as usize];
    ctx.load_texture(
        "camera-thumbnail",
        egui::ColorImage::from_rgb(size, &rgb),
        egui::TextureOptions::LINEAR,
    )
}

/// The shape the thumbnail is drawn in: the photo's when they are nearly the same, as
/// a 1616 × 1080 preview of a 3:2 frame holds all of that frame in rounded sizes.
/// Another shape is another crop (16:9 in camera), drawn as it is.
fn drawn_size(texture: Vec2, photo: Option<[f32; 2]>) -> Vec2 {
    match photo {
        Some([w, h])
            if w > 0. && h > 0. && (texture.x / texture.y / (w / h) - 1.).abs() <= 0.02 =>
        {
            Vec2::new(texture.y * w / h, texture.y)
        }
        _ => texture,
    }
}

/// `im` at most [`MAX_EDGE`] long.
fn bounded(im: image::RgbImage) -> image::RgbImage {
    let edge = im.width().max(im.height());
    if edge <= MAX_EDGE {
        return im;
    }
    let k = MAX_EDGE as f32 / edge as f32;
    image::imageops::resize(
        &im,
        (im.width() as f32 * k) as u32,
        (im.height() as f32 * k) as u32,
        image::imageops::FilterType::Triangle,
    )
}

impl Editor {
    /// Reads the open photo's camera thumbnail unless it was, or is being, read.
    pub(super) fn start_camera_thumbnail(&mut self) {
        if self.document.camera_thumbnail.is_some()
            || self.document.camera_thumbnail_task.is_running()
        {
            return;
        }
        let Some(path) = self.document.path.clone() else {
            self.document.camera_thumbnail = Some(Err("no photo is open".into()));
            return;
        };
        let (generation, _) = self.document.camera_thumbnail_task.start();
        let id = self.load.id();
        super::task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let result = crate::camera_look::preview_srgb(&path)
                    .map(|mut preview| {
                        preview.image = bounded(preview.image);
                        preview
                    })
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(Event::CameraThumbnail {
                    id,
                    generation,
                    result,
                });
            },
            move |tx, message| {
                let _ = tx.send(Event::CameraThumbnail {
                    id,
                    generation,
                    result: Err(message),
                });
            },
        );
    }

    pub(super) fn camera_thumbnail_ready(
        &mut self,
        ctx: &egui::Context,
        generation: u64,
        result: Result<crate::camera_look::CameraPreview, String>,
    ) {
        if generation != self.document.camera_thumbnail_task.id() {
            return;
        }
        self.document.camera_thumbnail_task.finish(generation);
        let monitor = self.view.display_profile();
        self.document.camera_thumbnail =
            Some(result.map(|preview| Thumbnail::new(ctx, preview.image, preview.look, monitor)));
    }

    /// The view: the thumbnail, or that it is being read, or why there is none.
    pub(super) fn camera_thumbnail_ui(&mut self, ui: &egui::Ui, area: Rect) {
        let monitor = self.view.display_profile();
        let photo = self
            .document
            .full()
            .map(|im| crate::model::image_frame::ImageFrame::new(im).size());
        let mut label = LABEL.to_owned();
        match &mut self.document.camera_thumbnail {
            Some(Ok(thumbnail)) => {
                thumbnail.follow(ui.ctx(), monitor);
                label = thumbnail.label();
                let texture = &thumbnail.texture;
                let size = drawn_size(texture.size_vec2(), photo);
                let k = (area.width() / size.x).min(area.height() / size.y);
                let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.));
                let rect = Rect::from_center_size(area.center(), size * k);
                ui.painter().image(texture.id(), rect, uv, Color32::WHITE);
            }
            state => {
                let message = match state {
                    Some(Err(why)) => format!("No camera thumbnail: {why}"),
                    _ => "Reading the camera thumbnail…".to_owned(),
                };
                ui.painter().text(
                    area.center(),
                    egui::Align2::CENTER_CENTER,
                    message,
                    egui::FontId::proportional(text::TITLE),
                    theme::palette(ui.ctx()).tone(Tone::FAINT),
                );
            }
        }
        super::before_after::badge(ui, area, &label);
    }

    /// The view on or off: what its key and menu items do.
    pub(super) fn toggle_camera_thumbnail(&mut self) {
        self.set_compare(self.view.compare.toggled(Compare::CameraThumbnail));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(ctx: &egui::Context) -> Editor {
        Editor::with_context(ctx, None, crate::app::session::Session::default(), None)
    }

    #[test]
    fn a_rounded_preview_is_drawn_in_the_photos_shape_and_another_crop_in_its_own() {
        let preview = Vec2::new(1616., 1080.);
        assert_eq!(
            drawn_size(preview, Some([9504., 6336.])),
            Vec2::new(1620., 1080.)
        );
        let wide = Vec2::new(1920., 1080.);
        assert_eq!(drawn_size(wide, Some([9504., 6336.])), wide);
        assert_eq!(drawn_size(preview, None), preview);
    }

    #[test]
    fn it_toggles_like_before_alone_and_leaves_with_the_photo() {
        let ctx = egui::Context::default();
        let mut e = editor(&ctx);
        e.toggle_camera_thumbnail();
        assert_eq!(e.view.compare, Compare::CameraThumbnail);
        assert!(e.view.compare.before_only(), "the panels are disabled");
        assert_eq!(
            e.effective_recipe(),
            *e.document.edit.recipe(),
            "the photo is still the edit, not Before"
        );
        assert!(!e.view.compare.two_up());
        e.toggle_camera_thumbnail();
        assert_eq!(e.view.compare, Compare::Off);
        e.toggle_camera_thumbnail();
        e.view.clear_document();
        assert_eq!(e.view.compare, Compare::Off);
    }

    #[test]
    fn without_a_photo_it_says_so() {
        let ctx = egui::Context::default();
        let mut e = editor(&ctx);
        e.toggle_camera_thumbnail();
        assert!(matches!(&e.document.camera_thumbnail, Some(Err(_))));
    }

    #[test]
    fn a_failure_clears_the_task_and_a_stale_result_is_ignored() {
        let ctx = egui::Context::default();
        let mut e = editor(&ctx);
        let (first, _) = e.document.camera_thumbnail_task.start();
        let (second, _) = e.document.camera_thumbnail_task.start();
        let preview = || crate::camera_look::CameraPreview {
            image: image::RgbImage::new(4, 2),
            look: None,
        };
        e.camera_thumbnail_ready(&ctx, first, Ok(preview()));
        assert!(e.document.camera_thumbnail.is_none(), "stale");
        assert!(e.document.camera_thumbnail_task.is_running());
        e.camera_thumbnail_ready(&ctx, second, Err("no preview".into()));
        assert!(!e.document.camera_thumbnail_task.is_running());
        assert!(matches!(&e.document.camera_thumbnail, Some(Err(w)) if w == "no preview"));
        // Read once: asking again keeps what is known.
        e.start_camera_thumbnail();
        assert!(!e.document.camera_thumbnail_task.is_running());
    }

    #[test]
    fn a_read_thumbnail_is_shown_in_the_viewport() {
        let ctx = egui::Context::default();
        let mut e = editor(&ctx);
        let (generation, _) = e.document.camera_thumbnail_task.start();
        let preview = crate::camera_look::CameraPreview {
            image: image::RgbImage::new(4, 2),
            look: Some("Classic Chrome".into()),
        };
        e.camera_thumbnail_ready(&ctx, generation, Ok(preview));
        assert_eq!(
            e.document
                .camera_thumbnail
                .as_ref()
                .and_then(|t| t.as_ref().ok())
                .map(|t| (t.texture.size(), t.label())),
            Some((
                [4, 2],
                "Camera thumbnail: Classic Chrome, embedded JPEG, no edits".to_owned()
            ))
        );
        e.module = crate::app::Module::Develop;
        e.view.compare = Compare::CameraThumbnail;
        let mut output = ctx.run_ui(Default::default(), |ui| {
            e.viewport_ui(ui);
        });
        output.textures_delta.clear();
        assert!(!output.shapes.is_empty());
    }

    /// A monitor profile chosen while the view is open converts it again.
    #[test]
    fn the_thumbnail_follows_the_monitor() {
        let ctx = egui::Context::default();
        let mut t = Thumbnail::new(&ctx, image::RgbImage::new(4, 2), None, None);
        let before = t.texture.id();
        t.follow(&ctx, None);
        assert_eq!(t.texture.id(), before, "the same monitor keeps the texture");
        let other = PathBuf::from("other.icc");
        t.follow(&ctx, Some(other.clone()));
        assert_ne!(t.texture.id(), before);
        assert_eq!(t.monitor, Some(other));
    }

    /// Shift+\ arrives as | on most layouts and as a shifted \ on others; \ alone is
    /// still Before.
    #[test]
    fn shift_backslash_is_the_camera_thumbnail_and_backslash_stays_before() {
        for (key, modifiers, expected) in [
            (
                egui::Key::Pipe,
                egui::Modifiers::SHIFT,
                Compare::CameraThumbnail,
            ),
            (
                egui::Key::Backslash,
                egui::Modifiers::SHIFT,
                Compare::CameraThumbnail,
            ),
            (
                egui::Key::Backslash,
                egui::Modifiers::NONE,
                Compare::BeforeOnly,
            ),
        ] {
            let ctx = egui::Context::default();
            let mut e = editor(&ctx);
            let input = egui::RawInput {
                events: vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::Key {
                        key,
                        physical_key: Some(egui::Key::Backslash),
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                ],
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |_| e.develop_shortcuts(&ctx));
            output.textures_delta.clear();
            assert_eq!(e.view.compare, expected, "{key:?} {modifiers:?}");
        }
    }
}
