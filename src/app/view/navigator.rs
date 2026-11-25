//! The zoom of the photo views, shared by Develop and the Library's Loupe, and
//! the slider that sets it, in place of Lightroom's Navigator (its minimap is
//! `minimap`'s). One `Zoom` is the view's zoom wherever the photo is shown, so
//! Develop, the Loupe and its JPEG/TIFF/PNG view all keep the same level and
//! place.
use crate::app::ui::design::{Tone, radius, text};
use crate::app::ui::theme;
use crate::catalog::PhotoId;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

/// The closest zoom the slider and Cmd+= reach: 1600%, or twice Fit for a photo
/// tiny beside the view; and the farthest Cmd+- reaches below Fit, for a photo
/// smaller than the view.
const MAX_LEVEL: f32 = 16.;
const MIN_LEVEL: f32 = 1. / 16.;
/// The closest zoom for a photo that fits at `fit`; the slider's end.
fn max_level(fit: f32) -> f32 {
    MAX_LEVEL.max(fit * 2.)
}
/// How much more than the view a 1:1 region renders on each side, as a
/// fraction of the view's width and height.
const REGION_MARGIN: f32 = 0.25;
/// The longest edge a region with its margin renders at: the texture size
/// every GPU supports (wgpu's default limit).
const REGION_EDGE: u32 = 8192;
/// Cmd+= and Cmd+- zoom by this factor, half a doubling, so each press looks like
/// the same step whatever the level.
const STEP: f32 = std::f32::consts::SQRT_2;

/// The level one zoom step in (`up`) or out from `current` reaches, for a photo
/// that fits at `fit`; 0 is Fit. A step across 100% stops there, and one that
/// reaches the fitted size fits, going either way. Zooming out never enlarges:
/// from Fit or above it stops at Fit, and below Fit (a photo smaller than the
/// view, at 100%) it goes on down, to `MIN_LEVEL`.
pub(in crate::app) fn stepped(current: f32, fit: f32, up: bool) -> f32 {
    const NEAR: f32 = 1e-3;
    let next = if up { current * STEP } else { current / STEP };
    let at_100 = if up {
        current < 1. - NEAR && next > 1. + NEAR
    } else {
        current > 1. + NEAR && next < 1. - NEAR
    };
    let below_fit = current < fit * (1. - NEAR);
    let at_fit = if up {
        below_fit && next >= fit * (1. - NEAR)
    } else {
        !below_fit && next <= fit * (1. + NEAR)
    };
    // A step reaching both stops at the nearer one it moves to: going in, the
    // lower; going out, Fit when it lies between, and 100% from a level
    // already at Fit's size.
    let fit_first = if up {
        fit < 1.
    } else {
        fit > 1. && current > fit * (1. + NEAR)
    };
    match (at_100, at_fit) {
        (true, true) => return if fit_first { 0. } else { 1. },
        (true, false) => return 1.,
        (false, true) => return 0.,
        (false, false) => {}
    }
    // At a limit, a step stays put rather than going the other way.
    let next = next.clamp(MIN_LEVEL.min(fit), max_level(fit));
    if (up && next < current) || (!up && next > current) {
        current
    } else {
        next
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct Zoom {
    /// Zoomed in rather than fitted.
    pub on: bool,
    /// Screen pixels per image pixel when zoomed in (1 = 100%).
    pub level: f32,
    /// The centre of the view, as a fraction of the photo's width and height.
    pub pan: [f32; 2],
}
impl Default for Zoom {
    fn default() -> Self {
        Self {
            on: false,
            level: 1.,
            pan: [0.5, 0.5],
        }
    }
}
impl Zoom {
    /// Fitted, with `level` from the session as the level a click zooms to:
    /// 100% when there is none, and within the levels a step reaches.
    pub(in crate::app) fn from_session(level: Option<f32>) -> Self {
        let level = level
            .filter(|l| l.is_finite() && *l > 0.)
            .map_or(1., |l| l.clamp(MIN_LEVEL, MAX_LEVEL));
        Self {
            level,
            ..Self::default()
        }
    }
    /// Screen rectangle of a whole photo of `size` image pixels laid out in `area`:
    /// fitted in Fit, otherwise `level` screen pixels per image pixel around `pan`,
    /// centred when smaller than `area`. `ppp` is pixels per point.
    pub(crate) fn photo_rect(&self, area: egui::Rect, size: egui::Vec2, ppp: f32) -> egui::Rect {
        if !self.on {
            let k = (area.width() / size.x).min(area.height() / size.y);
            return egui::Rect::from_center_size(area.center(), size * k);
        }
        let size = size * (self.level / ppp);
        let place = |pan: f32, lo: f32, len: f32, size: f32| {
            if size <= len {
                lo + (len - size) / 2.
            } else {
                (lo + len / 2. - pan * size).clamp(lo + len - size, lo)
            }
        };
        let min = egui::Pos2::new(
            place(self.pan[0], area.left(), area.width(), size.x),
            place(self.pan[1], area.top(), area.height(), size.y),
        );
        egui::Rect::from_min_size(min, size)
    }
    /// The part of a photo `size` image pixels in view, as fractions `[x, y, w,
    /// h]` of it, in a view `viewport` pixels in size: placed as `photo_rect`
    /// places it, at any level. None at Fit, or when all of it shows.
    pub(crate) fn shown(&self, viewport: Vec2, size: Vec2) -> Option<[f32; 4]> {
        if !self.on {
            return None;
        }
        let drawn = size * self.level;
        let part = |pan: f32, len: f32, drawn: f32| {
            let w = (len / drawn).min(1.);
            ((pan - w / 2.).clamp(0., 1. - w), w)
        };
        let (x, w) = part(self.pan[0], viewport.x, drawn.x);
        let (y, h) = part(self.pan[1], viewport.y, drawn.y);
        (w < 1. || h < 1.).then_some([x, y, w, h])
    }
    /// The 1:1 region `[x, y, w, h]` of a photo `width` × `height` image pixels
    /// that a view `viewport` pixels in size shows when zoomed to 100% or more;
    /// None below 100%, where the whole photo is rendered at the zoomed size.
    pub(crate) fn region(&self, viewport: Vec2, width: u32, height: u32) -> Option<[u32; 4]> {
        if !self.on || self.level < 1. {
            return None;
        }
        let z = self.level;
        let w = ((viewport.x / z).ceil() as u32).clamp(1, width);
        let h = ((viewport.y / z).ceil() as u32).clamp(1, height);
        let x = (self.pan[0] * width as f32 - w as f32 / 2.)
            .round()
            .clamp(0., (width - w) as f32) as u32;
        let y = (self.pan[1] * height as f32 - h as f32 / 2.)
            .round()
            .clamp(0., (height - h) as f32) as u32;
        Some([x, y, w, h])
    }
    /// The 1:1 region to render for a view `viewport` pixels in size, as
    /// [`Self::region`] but with a margin around it, so a drag moves pixels
    /// already rendered instead of rendering on every frame: on a GPU that is
    /// not much faster than the frames, that work held back the frames shown.
    /// `last`, the region last asked for, is kept while it still covers the
    /// view and is no larger than a new one would be.
    pub(crate) fn render_region(
        &self,
        viewport: Vec2,
        width: u32,
        height: u32,
        last: Option<[u32; 4]>,
    ) -> Option<[u32; 4]> {
        let [x, y, w, h] = self.region(viewport, width, height)?;
        let grow = |at: u32, len: u32, total: u32| {
            let margin = (len as f32 * REGION_MARGIN).round() as u32;
            let grown = (len + 2 * margin).min(REGION_EDGE.max(len)).min(total);
            let start = (at + len / 2).saturating_sub(grown / 2).min(total - grown);
            (start, grown)
        };
        let (gx, gw) = grow(x, w, width);
        let (gy, gh) = grow(y, h, height);
        let covers = |[lx, ly, lw, lh]: [u32; 4]| {
            lx <= x
                && ly <= y
                && lx + lw >= x + w
                && ly + lh >= y + h
                && lx + lw <= width
                && ly + lh <= height
                && lw <= gw
                && lh <= gh
        };
        Some(last.filter(|l| covers(*l)).unwrap_or([gx, gy, gw, gh]))
    }
    /// Lightroom's click on the photo: from Fit, zooms in keeping `pos` (on the
    /// photo drawn at `rect` in `area`, `size` image pixels) under the pointer;
    /// zoomed in, back to Fit.
    pub(crate) fn toggle_at(&mut self, pos: Pos2, rect: Rect, area: Rect, size: Vec2, ppp: f32) {
        if !self.on {
            let point = (pos - rect.min) / rect.size();
            let size = size * (self.level / ppp);
            let origin = pos - point * size;
            let pan = (area.center() - origin) / size;
            self.pan = [pan.x.clamp(0., 1.), pan.y.clamp(0., 1.)];
        }
        self.on = !self.on;
    }
    /// Sets a level; 0 fits.
    pub(crate) fn set(&mut self, level: f32) {
        self.on = level > 0.;
        if self.on {
            self.level = level;
        }
    }
}

/// A minimap click or drag moving the view: the pan eases to where it points
/// instead of jumping there. A point on the minimap is twenty or more on the
/// photo, so a mouse reporting whole points, and frames that get no motion
/// between others that get several, would move the photo in uneven leaps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct Glide {
    target: [f32; 2],
    /// The pan this glide last set: anything else moving it (a hand drag)
    /// ends the glide.
    at: [f32; 2],
    /// The photo it pans. Moving on keeps the zoom and pan, so the next photo
    /// would otherwise glide on to a point chosen on this one.
    photo: Option<PhotoId>,
}
impl Glide {
    /// The time constant of the ease, in seconds.
    const TAU: f32 = 0.05;
    pub(in crate::app) fn to(target: [f32; 2], from: [f32; 2], photo: Option<PhotoId>) -> Self {
        Self {
            target,
            at: from,
            photo,
        }
    }
    /// Where the view is going, as a fraction of the photo's width and height.
    pub(in crate::app) fn target(&self) -> [f32; 2] {
        self.target
    }
    /// Moves `zoom`'s pan on `photo` `dt` seconds further towards the target.
    /// Returns whether it has further to go; not once something else moved the
    /// pan, or on another photo.
    pub(in crate::app) fn step(
        &mut self,
        zoom: &mut Zoom,
        photo: Option<PhotoId>,
        dt: f32,
    ) -> bool {
        const NEAR: f32 = 1e-5;
        let moved = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs().max((a[1] - b[1]).abs());
        if !zoom.on || photo != self.photo || moved(zoom.pan, self.at) > NEAR {
            return false;
        }
        let k = 1. - (-dt.max(0.) / Self::TAU).exp();
        let pan = [0, 1].map(|i| zoom.pan[i] + (self.target[i] - zoom.pan[i]) * k);
        // Within a tenth of a pixel of a photo 10,000 pixels across.
        let arrived = moved(pan, self.target) < 1e-5;
        zoom.pan = if arrived { self.target } else { pan };
        self.at = zoom.pan;
        !arrived
    }
}

/// A photo's texture as the minimap draws it.
#[derive(Clone, Copy, Debug)]
pub(in crate::app) struct Photo {
    pub(in crate::app) texture: egui::TextureId,
    /// Its size as shown.
    pub(in crate::app) size: Vec2,
    /// The quarter turns clockwise it is drawn with: a render from before a
    /// turn, until one after it lands.
    pub(in crate::app) turns: u8,
}
impl Photo {
    /// A texture of `size` drawn `turns` quarter turns clockwise, at its
    /// turned size.
    pub(in crate::app) fn turned(texture: egui::TextureId, size: Vec2, turns: u8) -> Self {
        let size = if turns % 2 == 1 { size.yx() } else { size };
        Self {
            texture,
            size,
            turns,
        }
    }
}

/// The zoom levels the slider's menu offers besides Fit, as Lightroom's
/// Navigator menu does. 12.5% and 25% are below Fit for most photos, which
/// leaves room around the frame.
const PRESETS: [(f32, &str); 8] = [
    (0.125, "12.5%"),
    (0.25, "25%"),
    (0.5, "50%"),
    (1., "100%"),
    (2., "200%"),
    (4., "400%"),
    (8., "800%"),
    (16., "1600%"),
];

/// A level chosen on the zoom slider; 0 is Fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) enum ZoomChoice {
    /// Dragged or stepped: the photo follows at once, as an eased zoom would
    /// trail behind every move of the knob.
    Moved(f32),
    /// Picked from the level's menu: eased, as the zoom keys are.
    Picked(f32),
}
impl ZoomChoice {
    pub(in crate::app) fn level(self) -> f32 {
        match self {
            Self::Moved(l) | Self::Picked(l) => l,
        }
    }
}

/// The zoom slider, in a toolbar button's shape `height` high: `−`, a track from
/// Fit to 1600% on a log scale, `+`, and the level shown, which opens a menu of
/// Fit and [`PRESETS`]. `zoom` is on a photo that fits at `fit` screen pixels
/// per image pixel (a nominal level until the photo's size is known).
pub(in crate::app) fn zoom_slider(
    ui: &mut egui::Ui,
    zoom: Zoom,
    fit: Option<f32>,
    width: f32,
    height: f32,
) -> Option<ZoomChoice> {
    const BUTTON: f32 = 20.;
    const LABEL: f32 = 52.;
    let palette = theme::palette(ui.ctx());
    let fit = fit.filter(|f| f.is_finite() && *f > 0.).unwrap_or(0.1);
    let max = max_level(fit);
    let current = if zoom.on { zoom.level } else { fit };
    let span = (max / fit).ln();
    let to_t = |level: f32| ((level / fit).ln() / span).clamp(0., 1.);
    let from_t = |t: f32| fit * (span * t).exp();
    let (outer, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    ui.painter()
        .rect_filled(outer, radius::CONTROL, palette.tone(Tone::CARD));
    // − track + level, packed: the level reads as part of the control.
    let rect = outer.shrink2(Vec2::new(6., 0.));
    let minus = Rect::from_min_size(rect.min, Vec2::new(BUTTON, height));
    let label = Rect::from_min_size(
        Pos2::new(rect.right() - LABEL, rect.top()),
        Vec2::new(LABEL, height),
    );
    let plus = Rect::from_min_size(
        Pos2::new(label.left() - BUTTON, rect.top()),
        Vec2::new(BUTTON, height),
    );
    let track = Rect::from_min_max(
        Pos2::new(minus.right() + 7., rect.center().y - 1.5),
        Pos2::new(plus.left() - 7., rect.center().y + 1.5),
    );
    let button = |ui: &mut egui::Ui, at: Rect, id: &str| {
        ui.interact(at, ui.id().with(id), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
    };
    let out = button(ui, minus, "zoom-out").on_hover_text("Zoom out · Cmd+-");
    let into = button(ui, plus, "zoom-in").on_hover_text("Zoom in · Cmd+=");
    let menu = button(ui, label.with_max_x(outer.right()), "zoom-level");
    menu.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Zoom level"));
    let drag = ui
        .interact(
            track.expand2(Vec2::new(8., 10.)),
            ui.id().with("zoom-track"),
            Sense::click_and_drag(),
        )
        .on_hover_text("Zoom · Cmd+1 fits, Cmd+0 is 100%");
    let mut chosen = None;
    if out.clicked() {
        chosen = Some(ZoomChoice::Moved(stepped(current, fit, false)));
    }
    if into.clicked() {
        chosen = Some(ZoomChoice::Moved(stepped(current, fit, true)));
    }
    if (drag.dragged() || drag.clicked())
        && let Some(p) = drag.interact_pointer_pos()
    {
        let t = ((p.x - track.left()) / track.width()).clamp(0., 1.);
        // 100% is easy to hit: the knob holds there for a few points.
        let at_100 = fit < 1. && (track.left() + to_t(1.) * track.width() - p.x).abs() < 4.;
        chosen = Some(ZoomChoice::Moved(if t <= 0.005 {
            0.
        } else if at_100 {
            1.
        } else {
            from_t(t)
        }));
    }
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&menu));
    let menu_hovered = menu.hovered() || open;
    let menu = if open {
        menu
    } else {
        menu.on_hover_text("Zoom level")
    };
    egui::Popup::menu(&menu).show(|ui| {
        crate::app::ui::widgets::menu_width(ui, 120.);
        ui.spacing_mut().item_spacing.y = 0.;
        let near = |level: f32| zoom.on && (zoom.level / level - 1.).abs() < 1e-3;
        if crate::app::ui::widgets::menu_item(ui, "Fit", "", true, !zoom.on) {
            chosen = Some(ZoomChoice::Picked(0.));
            ui.close();
        }
        crate::app::ui::widgets::menu_separator(ui);
        for (level, name) in PRESETS {
            if crate::app::ui::widgets::menu_item(ui, name, "", true, near(level)) {
                chosen = Some(ZoomChoice::Picked(level));
                ui.close();
            }
        }
    });
    let chosen_level = chosen.map(ZoomChoice::level);
    let painter = ui.painter();
    let stroke = |hovered: bool| {
        Stroke::new(
            1.5,
            palette.tone(if hovered { Tone::STRONG } else { Tone::LABEL }),
        )
    };
    let d = 5.;
    let c = minus.center();
    painter.line_segment(
        [c - Vec2::new(d, 0.), c + Vec2::new(d, 0.)],
        stroke(out.hovered()),
    );
    let c = plus.center();
    painter.line_segment(
        [c - Vec2::new(d, 0.), c + Vec2::new(d, 0.)],
        stroke(into.hovered()),
    );
    painter.line_segment(
        [c - Vec2::new(0., d), c + Vec2::new(0., d)],
        stroke(into.hovered()),
    );
    let shown = chosen_level.map_or(current, |l| if l == 0. { fit } else { l });
    let x = track.left() + to_t(shown) * track.width();
    painter.rect_filled(track, 2., palette.tone(Tone::OUTLINE));
    painter.rect_filled(
        Rect::from_min_max(track.min, Pos2::new(x, track.bottom())),
        2.,
        palette.accent(),
    );
    // A tick at 100%, where it falls on the track.
    if fit < 1. {
        let x100 = track.left() + to_t(1.) * track.width();
        painter.line_segment(
            [
                Pos2::new(x100, track.top() - 3.),
                Pos2::new(x100, track.bottom() + 3.),
            ],
            Stroke::new(1., palette.tone(Tone::LABEL)),
        );
    }
    painter.circle_filled(Pos2::new(x, track.center().y), 6., Color32::WHITE);
    let text = if chosen_level.map_or(!zoom.on, |l| l == 0.) {
        "Fit".to_owned()
    } else {
        level_text(shown)
    };
    painter.text(
        label.left_center() + Vec2::new(4., 0.),
        egui::Align2::LEFT_CENTER,
        format!("{text} ▾"),
        egui::FontId::proportional(text::SMALL),
        palette.tone(if menu_hovered {
            Tone::STRONG
        } else {
            Tone::LABEL
        }),
    );
    chosen
}

/// A level as the slider names it: whole percent, or a tenth below 20%, where
/// whole percent would round 12.5% to 13%.
fn level_text(level: f32) -> String {
    let percent = level * 100.;
    if percent < 19.95 && (percent * 10.).round() % 10. != 0. {
        format!("{percent:.1}%")
    } else {
        format!("{percent:.0}%")
    }
}

#[cfg(test)]
mod tests {
    use super::stepped;

    /// Steps multiply the level, stop at 100% on the way across it, and fit once
    /// they reach the fitted size.
    #[test]
    fn zoom_steps_are_a_factor_that_stops_at_100_and_fit() {
        let fit = 0.2;
        let up = stepped(fit, fit, true);
        assert!((up - 0.2 * std::f32::consts::SQRT_2).abs() < 1e-5);
        assert_eq!(stepped(0.9, fit, true), 1.);
        assert_eq!(stepped(1.2, fit, false), 1.);
        assert!((stepped(1., fit, true) - std::f32::consts::SQRT_2).abs() < 1e-5);
        assert_eq!(stepped(0.25, fit, false), 0.);
        assert_eq!(stepped(16., fit, true), 16.);
        // A photo smaller than the view fits above 100%: zooming out reaches
        // 100%, and never enlarges it to Fit after that.
        assert_eq!(stepped(2., 2., false), 0.);
        assert_eq!(stepped(1.2, 1.2, false), 1.);
        assert!((stepped(1., 1.2, false) - 1. / std::f32::consts::SQRT_2).abs() < 1e-5);
        // Zooming in from below Fit steps on and stops at Fit, not past it
        // nor straight to it.
        assert_eq!(stepped(1., 1.2, true), 0.);
        let up = |l: f32, fit| stepped(l, fit, true);
        assert!((up(0.125, 0.5) - 0.125 * std::f32::consts::SQRT_2).abs() < 1e-5);
        assert_eq!(up(0.25, 0.3), 0.);
        assert!((up(0.125, 2.) - 0.125 * std::f32::consts::SQRT_2).abs() < 1e-5);
        assert_eq!(up(0.8, 2.), 1.);
        // Fit below 100%, both within the step: Fit comes first going in,
        // 100% going out.
        assert_eq!(up(0.8, 0.9), 0.);
        assert_eq!(stepped(1.1, 0.9, false), 1.);
        assert_eq!(stepped(1.3, 1.2, false), 0.);
        assert_eq!(stepped(0.07, 1.2, false), 1. / 16.);
        // A large photo stops at Fit, whether zoomed in at it or above it.
        assert_eq!(stepped(0.2, 0.2, false), 0.);
        assert_eq!(stepped(0.25, 0.2, false), 0.);
        // A huge one, fitting below the floor for small photos, still steps down
        // to Fit without turning back.
        assert!(stepped(0.04, 0.02, false) < 0.04);
        assert!(stepped(0.0625, 0.04, false) < 0.0625);
        assert_eq!(stepped(0.044, 0.04, false), 0.);
        // A tiny one zooms in to the slider's end, twice Fit, and stays there.
        assert!(stepped(14., 10., true) > 14.);
        assert_eq!(stepped(20., 10., true), 20.);
        assert_eq!(stepped(16., 0.2, true), 16.);
        // Across photos from huge to tiny and any level the slider or keys
        // reach, zooming in never shrinks and zooming out never enlarges.
        for fit in [0.01, 0.02, 0.04, 0.1, 0.2, 0.5, 0.9, 1., 1.2, 2., 10., 50.] {
            let mut level: f32 = 1. / 32.;
            while level <= 100. {
                let shown = |l: f32| if l == 0. { fit } else { l };
                let up = shown(stepped(level, fit, true));
                let down = shown(stepped(level, fit, false));
                assert!(up >= level * 0.999, "in from {level} at fit {fit}: {up}");
                assert!(
                    down <= level * 1.001,
                    "out from {level} at fit {fit}: {down}"
                );
                level *= 1.19;
            }
        }
    }

    /// The level beside the slider opens a menu of Fit and preset levels,
    /// some below Fit, and picking one zooms the photo to it.
    #[test]
    fn the_zoom_level_opens_a_menu_of_levels() {
        use crate::app::Editor;
        use crate::app::testing::{centre, labelled};
        use eframe::egui::{self, Pos2, Rect, Vec2};
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        ctx.all_styles_mut(|s| s.animation_time = 0.);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("session.json");
        let mut editor = Editor::with_context(&ctx, None, Default::default(), Some(file.clone()));
        editor.module = crate::app::Module::Develop;
        editor.document.set_image(crate::app::testing::blue_image());
        // Large enough a view that the photo fits above 12.5%.
        editor.view.viewport = Vec2::new(1000., 800.);
        let frame = |editor: &mut Editor, events: Vec<egui::Event>| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(500., 300.))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        editor.zoom_controls(ui, 32.)
                    });
                },
            );
            output.textures_delta.clear();
            output
                .platform_output
                .accesskit_update
                .expect("accessibility is on")
        };
        let click = |editor: &mut Editor, label: &str| {
            let tree = frame(editor, vec![]);
            let at = centre(labelled(&tree, label).unwrap_or_else(|| panic!("{label} shown")));
            for pressed in [true, false] {
                frame(
                    editor,
                    vec![
                        egui::Event::PointerMoved(at),
                        egui::Event::PointerButton {
                            pos: at,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
        };
        let fit = editor.fit_level().expect("the photo's size is known");
        assert!(fit > 0.125, "fits at {fit}");
        for (pick, level) in [("50%", Some(0.5)), ("12.5%", Some(0.125)), ("Fit", None)] {
            click(&mut editor, "Zoom level");
            click(&mut editor, pick);
            let zoom = editor.view.zoom;
            assert_eq!(zoom.on, level.is_some(), "{pick}");
            if let Some(level) = level {
                assert_eq!(zoom.level, level, "{pick}");
            }
        }
        // The level chosen is kept with the session, fitted or not: the next
        // launch opens at Fit, and a click on the photo zooms to it.
        editor.remember_zoom_level();
        let saved: crate::app::preferences::session::Session =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved.zoom_level, Some(0.125));
        let reopened = Editor::with_context(&ctx, None, saved, None);
        assert_eq!(reopened.view.zoom.level, 0.125);
        assert!(!reopened.view.zoom.on);
        // The first frame draws it as it was: nothing to render again.
        assert_eq!(reopened.view.zoom_key, (false, 0.125));
        // A saved level out of reach reads back within it and isn't written
        // again until the zoom changes; an unreadable one reads as 100%.
        for (saved, read) in [(Some(20.), 16.), (Some(-2.), 1.), (Some(f32::NAN), 1.)] {
            let session = crate::app::preferences::session::Session {
                zoom_level: saved,
                ..Default::default()
            };
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("session.json");
            let mut editor = Editor::with_context(&ctx, None, session, Some(file.clone()));
            assert_eq!(editor.view.zoom.level, read, "{saved:?}");
            editor.remember_zoom_level();
            assert!(!file.exists(), "{saved:?} written again unchanged");
        }
    }

    /// A saved level reads back within the levels a step reaches; none, or
    /// one that is no level, is 100%.
    #[test]
    fn a_saved_zoom_level_reads_back_within_reach() {
        use super::Zoom;
        let level = |l| Zoom::from_session(l).level;
        assert_eq!(level(Some(0.5)), 0.5);
        assert_eq!(level(None), 1.);
        assert_eq!(level(Some(f32::NAN)), 1.);
        assert_eq!(level(Some(-2.)), 1.);
        assert_eq!(level(Some(0.)), 1.);
        assert_eq!(level(Some(1e9)), 16.);
        assert_eq!(level(Some(1e-9)), 1. / 16.);
        assert!(!Zoom::from_session(Some(0.5)).on);
    }

    /// The zoom controls as a photo's toolbar draws them, at Fit, 50% and 1600%,
    /// each with the level's menu closed and open, for `RAWMAKASE_UI_SHOTS`
    /// ([`crate::app::ui::shots`]). The menu opens on a click, which a shot
    /// can't make, so it is drawn here.
    #[test]
    fn shots_of_the_zoom_level_menu() {
        use crate::app::Editor;
        use eframe::egui::{self, Vec2};
        use egui_kittest::kittest::Queryable;
        let Some(dir) = std::env::var_os("RAWMAKASE_UI_SHOTS").map(std::path::PathBuf::from) else {
            return;
        };
        for (name, level, open) in [
            ("zoom-fit", 0., false),
            ("zoom-fit-menu", 0., true),
            ("zoom-50", 0.5, false),
            ("zoom-12-menu", 0.125, true),
            ("zoom-1600", 16., false),
        ] {
            let mut harness = egui_kittest::Harness::builder()
                .with_size(Vec2::new(520., 360.))
                .with_pixels_per_point(2.)
                .wgpu()
                .build_ui_state(
                    |ui, editor: &mut Option<Editor>| {
                        let editor = editor.get_or_insert_with(|| {
                            let text = fastframe_text::detect();
                            crate::app::install_fonts(ui.ctx(), &text);
                            crate::app::ui::icons::install(ui.ctx());
                            let mut e =
                                Editor::with_context(ui.ctx(), None, Default::default(), None);
                            e.module = crate::app::Module::Develop;
                            e.view.last_fit = Some(0.2);
                            e.view.zoom.set(level);
                            e
                        });
                        let palette = crate::app::ui::theme::palette(ui.ctx());
                        egui::Panel::bottom("toolbar")
                            .frame(crate::app::develop::toolbar::photo_toolbar_frame(&palette))
                            .show_separator_line(false)
                            .show(ui, |ui| {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| editor.zoom_controls(ui, 32.),
                                );
                            });
                    },
                    None,
                );
            harness.run_steps(4);
            if open {
                harness.get_by_label("Zoom level").click();
                harness.run_steps(4);
            }
            let image = harness.render().expect("wgpu could not draw the shot");
            std::fs::create_dir_all(&dir).expect("the shots folder");
            image
                .save(dir.join(format!("{name}.png")))
                .expect("the shot");
        }
    }

    #[test]
    fn levels_are_named_in_whole_percent_but_below_20() {
        use super::level_text;
        assert_eq!(level_text(0.125), "12.5%");
        assert_eq!(level_text(0.25), "25%");
        assert_eq!(level_text(0.2), "20%");
        assert_eq!(level_text(1. / 3.), "33%");
        assert_eq!(level_text(0.1), "10%");
        assert_eq!(level_text(16.), "1600%");
    }

    /// A minimap drag eases the view to where it points, frame by frame, and
    /// gives way to anything else moving it.
    #[test]
    fn a_glide_eases_the_pan_to_its_target_and_gives_way() {
        use super::{Glide, PhotoId, Zoom};
        let mut zoom = Zoom {
            on: true,
            level: 1.,
            pan: [0.5, 0.5],
        };
        let photo = Some(PhotoId(1));
        let mut glide = Glide::to([0.7, 0.3], zoom.pan, photo);
        assert!(glide.step(&mut zoom, photo, 1. / 60.));
        let first = zoom.pan;
        assert!(first[0] > 0.5 && first[0] < 0.7 && first[1] < 0.5 && first[1] > 0.3);
        // It arrives in well under half a second.
        let mut frames = 1;
        while glide.step(&mut zoom, photo, 1. / 60.) {
            frames += 1;
            assert!(frames < 30, "still gliding: {:?}", zoom.pan);
        }
        assert_eq!(zoom.pan, [0.7, 0.3]);
        // A hand drag mid-glide ends it where the hand put the view.
        let mut glide = Glide::to([0.2, 0.2], zoom.pan, photo);
        glide.step(&mut zoom, photo, 1. / 60.);
        zoom.pan = [0.6, 0.6];
        assert!(!glide.step(&mut zoom, photo, 1. / 60.));
        assert_eq!(zoom.pan, [0.6, 0.6]);
        // As does going back to Fit.
        let mut glide = Glide::to([0.2, 0.2], zoom.pan, photo);
        zoom.on = false;
        assert!(!glide.step(&mut zoom, photo, 1. / 60.));
        // And moving on to another photo, which keeps the zoom and pan.
        zoom.on = true;
        let pan = zoom.pan;
        let mut glide = Glide::to([0.2, 0.2], pan, photo);
        assert!(!glide.step(&mut zoom, Some(PhotoId(2)), 1. / 60.));
        assert_eq!(zoom.pan, pan);
    }

    /// A 1:1 region renders with a margin around the view, kept while a drag
    /// stays within it and rendered again once the view leaves it; never past
    /// the photo or the largest texture a GPU takes.
    #[test]
    fn a_region_renders_with_a_margin_a_drag_stays_within() {
        use super::Zoom;
        use eframe::egui::Vec2;
        let mut zoom = Zoom {
            on: true,
            level: 1.,
            pan: [0.5, 0.5],
        };
        let view = Vec2::new(1000., 600.);
        let region = |zoom: &Zoom, last| zoom.render_region(view, 6000, 4000, last);
        let first = region(&zoom, None).unwrap();
        assert_eq!(first, [2250, 1550, 1500, 900]);
        // A drag of 200 pixels stays within the margin of 250.
        zoom.pan[0] += 200. / 6000.;
        assert_eq!(region(&zoom, Some(first)), Some(first));
        // One of 300 leaves it: a new region around the view.
        zoom.pan[0] = 0.5 + 300. / 6000.;
        assert_eq!(region(&zoom, Some(first)), Some([2550, 1550, 1500, 900]));
        // A region rendered at a lower zoom, larger than one at this zoom
        // needs, is not kept.
        zoom.pan = [0.5, 0.5];
        zoom.level = 2.;
        assert_eq!(region(&zoom, Some(first)), Some([2625, 1775, 750, 450]));
        // At a corner the margin stays on the photo.
        zoom.level = 1.;
        zoom.pan = [0., 1.];
        assert_eq!(region(&zoom, None), Some([0, 3100, 1500, 900]));
        // A view almost as large as the GPU's largest texture gets what margin
        // fits, and one larger than the photo the whole photo.
        let wide = zoom.render_region(Vec2::new(7000., 600.), 20000, 4000, None);
        assert_eq!(wide.map(|r| r[2]), Some(8192));
        let all = zoom.render_region(Vec2::new(7000., 5000.), 6000, 4000, None);
        assert_eq!(all, Some([0, 0, 6000, 4000]));
    }

    /// The part of the photo in view, wherever the zoom is: below 100% too, when
    /// the photo is larger than the view; none when it all shows.
    #[test]
    fn the_part_in_view_is_known_at_any_zoom() {
        use super::Zoom;
        use eframe::egui::Vec2;
        let photo = Vec2::new(6000., 4000.);
        let view = Vec2::new(1200., 800.);
        let half = Zoom {
            on: true,
            level: 0.5,
            pan: [0.5, 0.5],
        };
        let [x, y, w, h] = half.shown(view, photo).expect("a part in view");
        assert!((w - 0.4).abs() < 1e-5 && (h - 0.4).abs() < 1e-5);
        assert!((x - 0.3).abs() < 1e-5 && (y - 0.3).abs() < 1e-5);
        // Panned to a corner, it stops at the photo's edge.
        let corner = Zoom {
            pan: [0., 1.],
            ..half
        };
        let [x, y, ..] = corner.shown(view, photo).unwrap();
        assert!(x.abs() < 1e-5 && (y - 0.6).abs() < 1e-5);
        // At Fit, or zoomed out until it all shows, there is nothing to outline.
        assert_eq!(Zoom::default().shown(view, photo), None);
        let small = Zoom { level: 0.1, ..half };
        assert_eq!(small.shown(view, photo), None);
    }
}
