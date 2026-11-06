//! The magnifier, Aperture's loupe (named apart from Lightroom's Loupe view, the
//! Library's single photo): a lens that follows the pointer over the fitted photo
//! in Develop or the Library's Loupe and shows what is under it at 100% or more, so
//! focus and noise can be checked without zooming in and losing the framing. Shift+Z
//! or the toolbar shows or hides it; the toolbar's menu or the wheel over the photo
//! sets its level. It renders in its own lane of the
//! renderer: only a region a little larger than the circle, at full resolution, so
//! small pointer moves need no new render. Until that render lands, the fitted
//! preview is magnified in its place.
use crate::app::Editor;
use crate::app::develop::state::Picture;
use crate::app::ui::design::text;
use crate::rendered::ClipOverlay;
use crate::{camera_data::CameraImage, develop::Geometry, model::recipe::Recipe};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use std::{path::PathBuf, sync::Arc};

/// Screen pixels per image pixel the magnifier offers, as its menu names them; the
/// wheel steps through them.
const LEVELS: [(f32, &str); 3] = [(1., "100%"), (2., "200%"), (4., "400%")];
/// The toolbar's loupe control: its button, and its level menu naming the level
/// or, short of room, a chevron alone.
const BUTTON_WIDTH: f32 = 32.;
const LEVEL_WIDTH: f32 = 58.;
const LEVEL_NARROW_WIDTH: f32 = 22.;
pub(in crate::app) const CONTROLS_WIDTH: f32 = BUTTON_WIDTH + 1. + LEVEL_WIDTH;
pub(in crate::app) const CONTROLS_NARROW_WIDTH: f32 = BUTTON_WIDTH + 1. + LEVEL_NARROW_WIDTH;
/// The circle's diameter, in points.
const DIAMETER: f32 = 240.;
/// The rendered region is this much wider than the circle shows, so the pointer can
/// move a little before another render is needed.
const MARGIN: f32 = 1.6;

/// Whether the magnifier is shown, and at what level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct Magnifier {
    pub(in crate::app) on: bool,
    /// Index into `LEVELS`.
    level: usize,
    /// Wheel notches scrolled towards the next level.
    wheel: f32,
}
impl Default for Magnifier {
    fn default() -> Self {
        Self {
            on: false,
            level: 0,
            wheel: 0.,
        }
    }
}
impl Magnifier {
    pub(in crate::app) fn level(&self) -> f32 {
        LEVELS[self.level].0
    }
    /// The wheel over the photo this frame steps the level.
    pub(in crate::app) fn read_wheel(&mut self, ui: &egui::Ui) {
        let lines: f32 = ui
            .input(crate::app::develop::tools::brush_scroll::Scroll::read)
            .iter()
            .map(|s| s.lines)
            .sum();
        if lines != 0. {
            self.scroll(lines);
        }
    }
    /// A wheel scroll of `lines` notches: a whole notch either way steps the level.
    fn scroll(&mut self, lines: f32) {
        self.wheel += lines;
        if self.wheel >= 1. {
            self.level = (self.level + 1).min(LEVELS.len() - 1);
            self.wheel = 0.;
        } else if self.wheel <= -1. {
            self.level = self.level.saturating_sub(1);
            self.wheel = 0.;
        }
    }
}

/// What the magnifier was last rendered with: the same job is not rendered again.
#[derive(Clone)]
struct MagnifierJob {
    image: Arc<CameraImage>,
    recipe: Recipe,
    /// The AI Denoise source the render captured: a newer result renders again.
    ai: Option<crate::develop::AiSource>,
    region: [u32; 4],
    monitor: Option<PathBuf>,
    clipping: ClipOverlay,
}
impl PartialEq for MagnifierJob {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.image, &other.image)
            && self.recipe == other.recipe
            && self.ai == other.ai
            && self.region == other.region
            && self.monitor == other.monitor
            && self.clipping == other.clipping
    }
}

/// The magnifier's render: its own task and texture, like Before's.
#[derive(Default)]
pub(in crate::app) struct MagnifierPreview {
    pub(in crate::app) task: crate::app::task::Task,
    /// The last render, and the region of the photo it shows.
    pub(in crate::app) texture: Option<Picture>,
    shown: Option<[u32; 4]>,
    pending: Option<[u32; 4]>,
    submitted: Option<MagnifierJob>,
}
impl MagnifierPreview {
    /// Drops what the magnifier showed, e.g. when the photo changes.
    pub(in crate::app) fn clear(&mut self) {
        self.task.invalidate();
        self.texture = None;
        self.shown = None;
        self.submitted = None;
    }
}

/// The width of the loupe's body around the glass, its heel's corner radius and
/// its charcoal.
const BEZEL: f32 = 11.;
const HEEL_CORNER: f32 = 12.;
const BODY: Color32 = Color32::from_rgb(44, 44, 45);

/// A ring between `inner` and `outer` around `centre`, coloured per vertex: `color`
/// of the angle (0 at the top, clockwise) and of the edge, outer or not.
fn ring(
    painter: &egui::Painter,
    centre: Pos2,
    inner: f32,
    outer: f32,
    color: impl Fn(f32, bool) -> Color32,
) {
    const SEGMENTS: u32 = 128;
    let mut mesh = egui::Mesh::default();
    for i in 0..=SEGMENTS {
        let a = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        let d = Vec2::new(a.sin(), -a.cos());
        mesh.colored_vertex(centre + d * inner, color(a, false));
        mesh.colored_vertex(centre + d * outer, color(a, true));
        if i > 0 {
            let k = i * 2;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    painter.add(mesh);
}

/// Aperture's loupe, as a tribute: a flat charcoal body, round but for its
/// squared bottom-right heel, holding the glass of `radius` at `centre`. Drawn
/// under the glass.
fn paint_body(painter: &egui::Painter, centre: Pos2, radius: f32) {
    let outer = radius + BEZEL;
    let heel = Rect::from_min_max(centre, centre + Vec2::splat(outer));
    let corner = egui::CornerRadius {
        se: HEEL_CORNER as u8,
        ..Default::default()
    };
    // A soft shadow under the whole body, a little below it.
    for k in 0..10 {
        let grow = k as f32 * 1.5;
        let shade = Color32::from_black_alpha(12 - k);
        let below = Vec2::new(0., 6.);
        painter.circle_filled(centre + below, outer + grow, shade);
        painter.rect_filled(
            heel.translate(below).expand(grow),
            egui::CornerRadius {
                se: (HEEL_CORNER + grow) as u8,
                ..Default::default()
            },
            shade,
        );
    }
    painter.circle_filled(centre, outer, BODY);
    painter.rect_filled(heel, corner, BODY);
}

/// The glass's rim, its depth, the level in the heel as Aperture wrote it ("100
/// ▾") and the resize grip in its corner, drawn over the glass.
fn paint_trim(painter: &egui::Painter, centre: Pos2, radius: f32, level: f32) {
    let outer = radius + BEZEL;
    // The glass sits below the body: shade fading in towards its rim.
    ring(painter, centre, radius - 10., radius, |_, edge| {
        Color32::from_black_alpha(if edge { 80 } else { 0 })
    });
    painter.circle_stroke(
        centre,
        radius + 0.5,
        Stroke::new(1.5, Color32::from_gray(22)),
    );
    let corner = centre + Vec2::splat(outer);
    let ink = Color32::from_gray(185);
    painter.text(
        corner - Vec2::new(30., 13.),
        egui::Align2::RIGHT_CENTER,
        format!("{:.0} ▾", level * 100.),
        egui::FontId::proportional(text::SMALL),
        ink,
    );
    // Three parallel diagonals nested into the corner, shorter towards it.
    for d in [17., 12.5, 8.] {
        painter.line_segment(
            [corner - Vec2::new(d, 5.), corner - Vec2::new(5., d)],
            Stroke::new(1.2, Color32::from_gray(125)),
        );
    }
}

/// The region of a photo `size` image pixels wide and high to render for a circle
/// showing `visible` image pixels across around `centre`: `MARGIN` times wider,
/// moved in steps of a quarter of `visible` so that it changes only once the pointer
/// has moved that far, and kept inside the photo.
fn region_around(centre: [f32; 2], visible: f32, size: [u32; 2]) -> [u32; 4] {
    let step = (visible / 4.).max(1.);
    let side = |len: u32| ((visible * MARGIN).ceil() as u32).clamp(1, len);
    let (w, h) = (side(size[0]), side(size[1]));
    let origin = |c: f32, side: u32, len: u32| {
        let snapped = (c / step).round() * step;
        (snapped - side as f32 / 2.)
            .round()
            .clamp(0., (len - side) as f32) as u32
    };
    [
        origin(centre[0], w, size[0]),
        origin(centre[1], h, size[1]),
        w,
        h,
    ]
}

/// Where the lens looks: the image pixel under its centre, in a photo of `size`
/// image pixels shown at `rect`, kept far enough in that the lens shows only the
/// photo, and how many image pixels a point inside it spans (`k`).
pub(in crate::app) struct Aim {
    pub(in crate::app) centre: [f32; 2],
    k: f32,
    visible: f32,
}
impl Aim {
    /// None when the photo is smaller than the lens at `level`.
    pub(in crate::app) fn new(
        pos: Pos2,
        rect: Rect,
        size: [u32; 2],
        level: f32,
        ppp: f32,
    ) -> Option<Self> {
        let k = ppp / level;
        let visible = DIAMETER * k;
        if visible >= size[0].min(size[1]) as f32 {
            return None;
        }
        let half = visible / 2.;
        let centre = [
            ((pos.x - rect.left()) / rect.width() * size[0] as f32)
                .clamp(half, size[0] as f32 - half),
            ((pos.y - rect.top()) / rect.height() * size[1] as f32)
                .clamp(half, size[1] as f32 - half),
        ];
        Some(Self { centre, k, visible })
    }
    /// The region to render or read for it; see `region_around`.
    pub(in crate::app) fn region(&self, size: [u32; 2]) -> [u32; 4] {
        region_around(self.centre, self.visible, size)
    }
    /// Whether region `[x, y, w, h]`, in image pixels, covers the whole lens.
    fn covered_by(&self, [x, y, w, h]: [f32; 4]) -> bool {
        let half = self.visible / 2.;
        self.centre[0] - half >= x
            && self.centre[1] - half >= y
            && self.centre[0] + half <= x + w
            && self.centre[1] + half <= y + h
    }
}

/// Draws the lens at `pos`, aimed by `aim` at `level`: the `sharp` region (its
/// texture and `[x, y, w, h]` in image pixels) when it covers the lens, the whole
/// `fitted` preview of the photo of `size` image pixels meanwhile; then its bezel.
pub(in crate::app) fn paint_lens(
    ui: &egui::Ui,
    pos: Pos2,
    aim: &Aim,
    level: f32,
    sharp: Option<(egui::TextureId, [f32; 4])>,
    fitted: Option<egui::TextureId>,
    size: [u32; 2],
) {
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("magnifier"),
    ));
    let radius = DIAMETER / 2.;
    let palette = crate::app::ui::theme::palette(ui.ctx());
    paint_body(&painter, pos, radius);
    painter.circle_filled(pos, radius, palette.photo_backdrop());
    let source = sharp
        .filter(|(_, r)| aim.covered_by(*r))
        .or_else(|| fitted.map(|t| (t, [0., 0., size[0] as f32, size[1] as f32])));
    if let Some((texture, [x, y, w, h])) = source {
        let (centre, k) = (aim.centre, aim.k);
        let uv = |d: Vec2| Pos2::new((centre[0] + d.x * k - x) / w, (centre[1] + d.y * k - y) / h);
        let mut mesh = egui::Mesh::with_texture(texture);
        mesh.vertices.push(egui::epaint::Vertex {
            pos,
            uv: uv(Vec2::ZERO),
            color: Color32::WHITE,
        });
        const SEGMENTS: u32 = 96;
        for i in 0..=SEGMENTS {
            let a = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            let d = Vec2::angled(a) * radius;
            mesh.vertices.push(egui::epaint::Vertex {
                pos: pos + d,
                uv: uv(d),
                color: Color32::WHITE,
            });
            if i > 0 {
                mesh.add_triangle(0, i, i + 1);
            }
        }
        painter.add(mesh);
    }
    paint_trim(&painter, pos, radius, level);
    ui.ctx().set_cursor_icon(egui::CursorIcon::None);
}

impl Editor {
    /// The toolbar's loupe control, one split button as Aperture's loupe carries
    /// its level: the icon shows or hides the magnifier, the menu beside it sets the
    /// level, which it names when `wide`. Choosing a level shows the magnifier.
    /// Zoomed in it does nothing, as the photo is already magnified: both parts are
    /// disabled. Laid out right to left.
    pub(in crate::app) fn magnifier_controls(
        &mut self,
        ui: &mut egui::Ui,
        wide: bool,
        height: f32,
    ) {
        use crate::app::ui::widgets::{menu_item, toolbar_action_sized};
        let level = self.view.magnifier.level;
        let blocked = self.magnifier_blocked();
        let usable = blocked.is_none();
        let why = blocked.unwrap_or_default();
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = 1.;
            let (label, width) = if wide {
                (format!("{} ▾", LEVELS[level].1), LEVEL_WIDTH)
            } else {
                ("▾".to_owned(), LEVEL_NARROW_WIDTH)
            };
            let menu = toolbar_action_sized(ui, &label, Vec2::new(width, height), false, usable, 0);
            if usable {
                let menu = menu.on_hover_text("Magnifier level");
                egui::Popup::menu(&menu).show(|ui| {
                    ui.set_width(120.);
                    ui.spacing_mut().item_spacing.y = 0.;
                    for (i, (_, name)) in LEVELS.iter().enumerate() {
                        if menu_item(ui, name, "", true, i == level) {
                            self.view.magnifier.level = i;
                            if !self.view.magnifier.on {
                                self.toggle_magnifier();
                            }
                            ui.close();
                        }
                    }
                });
            } else {
                menu.on_hover_text(why);
            }
            let button = toolbar_action_sized(
                ui,
                "",
                Vec2::new(BUTTON_WIDTH, height),
                usable && self.view.magnifier.on,
                usable,
                4,
            );
            if !usable {
                button.on_hover_text(why);
            } else if button
                .on_hover_text("Magnifier: enlarge the photo under the pointer · Shift+Z")
                .clicked()
            {
                self.toggle_magnifier();
            }
        });
    }
    /// Why the magnifier can't be used now, if it can't: it works over the fitted
    /// photo, from full-resolution pixels (the edit's, or in the Library's Loupe the
    /// file's or an offline RAW's 1:1 preview).
    pub(in crate::app) fn magnifier_blocked(&self) -> Option<&'static str> {
        if self.view.zoom.on {
            return Some("Fit the photo to use the magnifier · Z or Cmd+1");
        }
        // The Loupe's own view magnifies the file, or an offline RAW's 1:1 preview.
        let nothing = self.module == crate::app::Module::Library
            && self.library.as_ref().is_some_and(|l| {
                l.loupe_open() && l.loupe_develops().is_none() && !l.loupe_zoomable()
            });
        nothing.then_some("Offline, with no 1:1 preview to magnify")
    }
    /// Shift+Z: shows or hides the magnifier, while it can be used.
    pub(in crate::app) fn toggle_magnifier(&mut self) {
        if self.magnifier_blocked().is_some() {
            return;
        }
        self.view.magnifier.on = !self.view.magnifier.on;
        if !self.view.magnifier.on {
            self.preview.magnifier.clear();
        }
    }
    /// Draws the magnifier over the fitted photo at `rect`, when it is on and the
    /// pointer is over the photo, and asks for its region.
    pub(in crate::app) fn magnifier_ui(&mut self, ui: &egui::Ui, hover: Option<Pos2>, rect: Rect) {
        let Some(pos) = hover.filter(|p| rect.contains(*p)) else {
            return;
        };
        let Some(image) = self.document.full().cloned() else {
            return;
        };
        let recipe = self.effective_recipe();
        let g = Geometry::new(&image, &recipe, 0);
        let size = [g.width, g.height];
        self.view.magnifier.read_wheel(ui);
        let level = self.view.magnifier.level();
        let Some(aim) = Aim::new(pos, rect, size, level, ui.ctx().pixels_per_point()) else {
            return;
        };
        // As a 100% region of the edit is rendered.
        let max_edge = self.render_edges(&g).fit;
        self.schedule_magnifier(image, recipe, aim.region(size), max_edge);
        let magnifier = &self.preview.magnifier;
        let sharp = magnifier
            .texture
            .as_ref()
            .zip(magnifier.shown)
            .map(|(t, r)| (t.id(), r.map(|v| v as f32)));
        let fitted = self.preview.texture.as_ref().map(|t| t.id());
        paint_lens(ui, pos, &aim, level, sharp, fitted, size);
    }
    /// Renders the magnifier's region, unless that job is already rendered or on
    /// its way.
    fn schedule_magnifier(
        &mut self,
        image: Arc<CameraImage>,
        recipe: Recipe,
        region: [u32; 4],
        max_edge: u32,
    ) {
        // Switched-off panels left out, as the edit's.
        let recipe = if recipe.panels.all_on() {
            recipe
        } else {
            recipe.as_rendered().into_owned()
        };
        let ai = self.shown_ai_source(&image, &recipe.ai_denoise);
        let job = MagnifierJob {
            image,
            recipe,
            ai,
            region,
            monitor: self.view.display_profile(),
            clipping: self.view.clipping.overlay(),
        };
        if self.preview.magnifier.submitted.as_ref() == Some(&job) {
            return;
        }
        let drawn = self.preview.presented();
        let magnifier = &mut self.preview.magnifier;
        let (id, cancel) = magnifier.task.start();
        magnifier.pending = Some(region);
        magnifier.submitted = Some(job.clone());
        self.renderer.submit(crate::app::worker::RenderJob {
            id,
            pane: crate::app::worker::Pane::Magnifier,
            image: job.image,
            max_edge,
            cancel,
            recipe: job.recipe,
            region: Some(region),
            monitor: job.monitor,
            clipping: job.clipping,
            navigator: false,
            thumbnail: false,
            samples: false,
            overlay: crate::app::worker::Overlay::None,
            ai: job.ai,
            drawn,
        });
    }
    /// A magnifier render arrived.
    pub(in crate::app) fn magnifier_rendered(
        &mut self,
        ctx: &egui::Context,
        preview: crate::app::worker::Preview,
        stage: crate::app::worker::RenderStage,
        task: u64,
    ) {
        let magnifier = &mut self.preview.magnifier;
        match preview {
            crate::app::worker::Preview::Pixels {
                image, display_rgb, ..
            } => {
                let size = [image.width as usize, image.height as usize];
                let image = egui::ColorImage::from_rgb(size, &display_rgb);
                Picture::upload(&mut magnifier.texture, ctx, "magnifier", image);
            }
            crate::app::worker::Preview::Texture { id, size, .. } => {
                magnifier.texture = Some(Picture::presented(id, size));
            }
        }
        magnifier.shown = magnifier.pending;
        if stage != crate::app::worker::RenderStage::Draft {
            magnifier.task.finish(task);
        }
    }
    /// An edit is about to render: a magnifier render still running would hold it up
    /// on the renderer's one thread, so it is cancelled and asked for again behind it.
    pub(in crate::app) fn yield_magnifier(&mut self) {
        let magnifier = &mut self.preview.magnifier;
        if magnifier.task.is_running() {
            magnifier.task.invalidate();
            magnifier.submitted = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_data::Metadata;

    /// Hovering the fitted photo with the magnifier on renders the region under it
    /// in the magnifier's lane, which then covers the circle; off or zoomed in,
    /// nothing is asked for.
    #[test]
    fn hovering_renders_the_region_under_the_magnifier() {
        let ctx = egui::Context::default();
        let mut editor = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let image = Arc::new(CameraImage {
            recovered: Default::default(),
            width: 800,
            height: 800,
            pixels: vec![[0.2; 3]; 640_000],
            metadata: Metadata {
                width: 800,
                height: 800,
                wb: [1.; 3],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        });
        editor.document.set_image(image);
        editor.module = crate::app::Module::Develop;
        editor.preview.texture = Some(
            ctx.load_texture(
                "photo",
                egui::ColorImage::filled([200, 200], Color32::GRAY),
                egui::TextureOptions::LINEAR,
            )
            .into(),
        );
        let frame = |editor: &mut Editor, events: Vec<egui::Event>| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    editor.viewport_ui(ui);
                },
            );
            output.textures_delta.clear();
        };
        let hover = || vec![egui::Event::PointerMoved(Pos2::new(100., 100.))];
        frame(&mut editor, hover());
        assert!(editor.preview.magnifier.submitted.is_none());

        editor.toggle_magnifier();
        // Frames go on while the pointer rests, as in the app: the edit's own render
        // goes first, and the magnifier asks again behind it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while editor.preview.magnifier.shown.is_none() && std::time::Instant::now() < deadline {
            frame(&mut editor, hover());
            std::thread::sleep(std::time::Duration::from_millis(20));
            editor.events(&ctx);
        }
        let asked = editor.preview.magnifier.submitted.clone().expect("a job");
        // 240 points across at 100%, with its margin, around the middle snapped to
        // steps of 60 pixels.
        assert_eq!(asked.region, [228, 228, 384, 384]);
        assert!(editor.preview.magnifier.texture.is_some());
        assert_eq!(editor.preview.magnifier.shown, Some(asked.region));

        // A small move needs no new render.
        let id = editor.preview.magnifier.task.id();
        frame(
            &mut editor,
            vec![egui::Event::PointerMoved(Pos2::new(103., 102.))],
        );
        assert_eq!(editor.preview.magnifier.task.id(), id);

        // Zoomed in, the magnifier is not drawn and asks for nothing.
        editor.preview.magnifier.clear();
        editor.view.zoom.on = true;
        frame(&mut editor, hover());
        frame(&mut editor, hover());
        assert!(editor.preview.magnifier.submitted.is_none());

        // Off, what it showed is dropped.
        editor.view.zoom.on = false;
        editor.toggle_magnifier();
        assert!(editor.preview.magnifier.texture.is_none());
    }

    #[test]
    #[ignore = "writes an image to look at"]
    fn render_the_loupe() -> anyhow::Result<()> {
        let ctx = egui::Context::default();
        crate::app::install_fonts(&ctx, &fastframe_text::TextRendering::platform_default());
        let mut textures = Default::default();
        let image = crate::app::ui::ui_snapshot::render(
            &ctx,
            &mut textures,
            Vec2::new(360., 340.),
            2.,
            3,
            |ui| {
                let r = ui.max_rect();
                // A sky over a building, to see the lens against light and dark.
                ui.painter()
                    .rect_filled(r, 0., Color32::from_rgb(110, 150, 190));
                ui.painter().rect_filled(
                    Rect::from_min_max(Pos2::new(0., 200.), r.max),
                    0.,
                    Color32::from_rgb(60, 58, 52),
                );
                let c = r.center() - Vec2::splat(12.);
                paint_body(ui.painter(), c, 120.);
                ui.painter()
                    .circle_filled(c, 120., Color32::from_rgb(140, 175, 210));
                paint_trim(ui.painter(), c, 120., 1.);
            },
        );
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/loupe.png");
        image.save(&out)?;
        Ok(())
    }

    /// The region is wider than the circle, stays put for small moves, and stays
    /// inside the photo.
    #[test]
    fn region_follows_the_pointer_in_steps() {
        let size = [6000, 4000];
        let r = region_around([3000., 2000.], 400., size);
        assert_eq!(r[2], 640);
        assert!(r[0] as f32 <= 3000. - 200. && (r[0] + r[2]) as f32 >= 3000. + 200.);
        assert_eq!(region_around([3040., 2030.], 400., size), r);
        assert_ne!(region_around([3060., 2000.], 400., size), r);
        let corner = region_around([10., 10.], 400., size);
        assert_eq!([corner[0], corner[1]], [0, 0]);
        let far = region_around([5999., 3999.], 400., size);
        assert_eq!([far[0] + far[2], far[1] + far[3]], size);
    }
}

#[cfg(test)]
#[path = "magnifier_editor_tests.rs"]
mod editor_tests;
