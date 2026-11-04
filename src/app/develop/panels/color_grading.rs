//! Lightroom's Color Grading panel. View buttons pick the 3-Way view (Midtones above,
//! Shadows and Highlights below) or one wheel large (Shadows, Midtones, Highlights or
//! Global). Dragging a wheel's puck sets hue and saturation: Shift keeps the drag to
//! hue or to saturation, whichever it moves along first, Cmd (Ctrl elsewhere) moves
//! the puck finely, and double-clicking resets the wheel. Each wheel's Luminance sits
//! under it, Blending and Balance below them all, and every number can be typed.
//!
//! The wheels edit the values the sliders before them did (hue 0–1, saturation 0–1,
//! luminance −1 to 1), so a grade renders the same however it was set.
use crate::app::develop::step_name::StepName;
use crate::app::ui::design::{Tone, radius, size, space, text};
use crate::app::ui::theme;
use crate::app::ui::widgets::{
    SliderStyle, set_edit_context, setting_slider, slider_styled, slider_with,
};
use crate::model::params::ParameterId;
use crate::model::recipe::Recipe;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use std::f32::consts::TAU;

/// What the panel shows: the three tonal wheels, or one wheel large.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum GradingView {
    #[default]
    ThreeWay,
    Single(Region),
}

/// The tonal range a wheel grades.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::app) enum Region {
    Shadows,
    Midtones,
    Highlights,
    Global,
}

impl Region {
    fn title(self) -> &'static str {
        match self {
            Region::Shadows => "Shadows",
            Region::Midtones => "Midtones",
            Region::Highlights => "Highlights",
            Region::Global => "Global",
        }
    }
    /// The word Lightroom's History starts this wheel's steps with: "Shadow Hue".
    fn step(self) -> &'static str {
        match self {
            Region::Shadows => "Shadow",
            Region::Midtones => "Midtone",
            Region::Highlights => "Highlight",
            Region::Global => "Global",
        }
    }
    /// The wheel's [hue, saturation, luminance].
    fn grade(self, r: &mut Recipe) -> &mut [f32; 3] {
        match self {
            Region::Shadows => &mut r.grading[0],
            Region::Midtones => &mut r.grading[1],
            Region::Highlights => &mut r.grading[2],
            Region::Global => &mut r.effects.global_grade,
        }
    }
}

/// The views in the order of Lightroom's buttons.
const VIEWS: [GradingView; 5] = [
    GradingView::ThreeWay,
    GradingView::Single(Region::Shadows),
    GradingView::Single(Region::Midtones),
    GradingView::Single(Region::Highlights),
    GradingView::Single(Region::Global),
];
/// The widest a wheel is in the 3-Way view, and on its own.
const SMALL_WHEEL: f32 = 128.;
const LARGE_WHEEL: f32 = 210.;
/// How far a Fine drag moves the puck for the pointer's movement.
const FINE: f32 = 0.1;
/// How far (in wheel radii) a constrained drag goes before it picks hue or saturation.
const LOCK_DISTANCE: f32 = 0.03;
/// Pressing this close (in points) to the puck picks it up where it is; pressing
/// elsewhere moves it under the pointer first.
const PUCK_GRAB: f32 = 8.;
/// Height of a region's caption and of each row under a small wheel.
const CAPTION: f32 = 16.;
const ROW: f32 = size::CONTROL;

/// The Color Grading panel's contents.
pub(in crate::app) fn color_grading_ui(
    ui: &mut egui::Ui,
    step: &mut StepName,
    r: &mut Recipe,
    view: &mut GradingView,
) {
    view_buttons(ui, view);
    ui.add_space(space::XS);
    match *view {
        GradingView::ThreeWay => three_way(ui, step, r),
        GradingView::Single(region) => single(ui, step, r, region),
    }
    ui.add_space(space::XS);
    set_edit_context(ui, "Color Grading");
    // The wheels' own sliders stay with them: their rails follow the wheel's color.
    for (id, default) in [
        (ParameterId::GradingBlending, 0.5),
        (ParameterId::GradingBalance, 0.),
    ] {
        setting_slider(ui, step, id, id.value_mut(r), default);
    }
}

fn view_buttons(ui: &mut egui::Ui, view: &mut GradingView) {
    let palette = theme::palette(ui.ctx());
    let size = Vec2::new(30., 24.);
    let gap = 4.;
    let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.), Sense::hover());
    let total = VIEWS.len() as f32 * (size.x + gap) - gap;
    let mut x = row.center().x - total / 2.;
    for option in VIEWS {
        let name = match option {
            GradingView::ThreeWay => "3-Way",
            GradingView::Single(region) => region.title(),
        };
        let rect = Rect::from_min_size(Pos2::new(x, row.center().y - size.y / 2.), size);
        x += size.x + gap;
        let response = ui
            .interact(rect, ui.id().with(("grading-view", name)), Sense::click())
            .on_hover_text(name);
        let selected = *view == option;
        if selected || response.hovered() {
            ui.painter().rect_filled(
                rect,
                radius::CONTROL,
                palette.tone(if selected { Tone::HOVER } else { Tone::OUTLINE }),
            );
        }
        let color = palette.tone(if selected || response.hovered() {
            Tone::STRONG
        } else {
            Tone::LABEL
        });
        view_icon(ui.painter(), option, rect.center(), color);
        if response.clicked() {
            *view = option;
        }
    }
}

/// Lightroom's button glyphs: three small circles, or one filled with the region's tone.
fn view_icon(painter: &egui::Painter, view: GradingView, c: Pos2, color: Color32) {
    let stroke = Stroke::new(1., color);
    let fill = match view {
        GradingView::ThreeWay => {
            for offset in [Vec2::new(0., -4.), Vec2::new(-5., 4.), Vec2::new(5., 4.)] {
                painter.circle_stroke(c + offset, 3.5, stroke);
            }
            return;
        }
        GradingView::Single(Region::Global) => {
            paint_disc(painter, c, 6., 24, 1);
            painter.circle_stroke(c, 6., stroke);
            return;
        }
        GradingView::Single(Region::Shadows) => Tone::BAR,
        GradingView::Single(Region::Midtones) => Tone::FAINT,
        GradingView::Single(Region::Highlights) => Tone::STRONG,
    };
    painter.circle(c, 6., theme::palette(painter.ctx()).tone(fill), stroke);
}

/// Midtones above, Shadows and Highlights side by side below, each with its numbers.
fn three_way(ui: &mut egui::Ui, step: &mut StepName, r: &mut Recipe) {
    let width = ui.available_width();
    let gap = 12.;
    let column = (width - gap) / 2.;
    let diameter = column.min(SMALL_WHEEL);
    let height = CAPTION + diameter + 2. * ROW;
    let (top, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let centre = Rect::from_center_size(
        Pos2::new(top.center().x, top.center().y),
        Vec2::new(column, height),
    );
    region_column(ui, step, centre, r, Region::Midtones, diameter);
    ui.add_space(space::S);
    let (bottom, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let left = Rect::from_min_size(bottom.min, Vec2::new(column, height));
    let right = Rect::from_min_size(
        Pos2::new(bottom.right() - column, bottom.top()),
        Vec2::new(column, height),
    );
    region_column(ui, step, left, r, Region::Shadows, diameter);
    region_column(ui, step, right, r, Region::Highlights, diameter);
}

/// A small wheel in `rect` under its caption, with its hue and saturation numbers and
/// its Luminance below.
fn region_column(
    ui: &mut egui::Ui,
    step: &mut StepName,
    rect: Rect,
    r: &mut Recipe,
    region: Region,
    diameter: f32,
) {
    let grade = region.grade(r);
    ui.painter().text(
        Pos2::new(rect.center().x, rect.top() + CAPTION / 2.),
        egui::Align2::CENTER_CENTER,
        region.title(),
        egui::FontId::proportional(text::SMALL),
        theme::palette(ui.ctx()).tone(Tone::SOFT),
    );
    let wheel_rect = Rect::from_min_size(
        Pos2::new(rect.center().x - diameter / 2., rect.top() + CAPTION),
        Vec2::splat(diameter),
    );
    wheel(ui, step, wheel_rect, grade, region);
    let fields = Rect::from_min_size(
        Pos2::new(rect.left(), wheel_rect.bottom()),
        Vec2::new(rect.width(), ROW),
    );
    hue_saturation_fields(ui, step, fields, grade, region);
    let rail = Rect::from_min_size(
        Pos2::new(rect.left(), fields.bottom()),
        Vec2::new(rect.width(), ROW),
    );
    luminance_rail(ui, step, rail, &mut grade[2], region);
}

/// One large wheel with Hue, Saturation and Luminance sliders.
fn single(ui: &mut egui::Ui, step: &mut StepName, r: &mut Recipe, region: Region) {
    let palette = theme::palette(ui.ctx());
    let grade = region.grade(r);
    let diameter = ui.available_width().min(LARGE_WHEEL);
    let (row, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), diameter), Sense::hover());
    wheel(
        ui,
        step,
        Rect::from_center_size(row.center(), Vec2::splat(diameter)),
        grade,
        region,
    );
    ui.add_space(space::XS);
    set_edit_context(ui, region.step());
    ui.push_id(("grade", region), |ui| {
        let mut degrees = grade[0] * 360.;
        let wheel = SliderStyle {
            hue_rail: true,
            ..SliderStyle::default()
        };
        slider_styled(ui, step, "Hue", &mut degrees, 0. ..=360., 0., wheel);
        if degrees != grade[0] * 360. {
            grade[0] = degrees / 360.;
        }
        let tint = hue_color(grade[0], 1.);
        slider_with(
            ui,
            step,
            "Saturation",
            &mut grade[1],
            0. ..=1.,
            0.,
            None,
            Some((palette.tone(Tone::OUTLINE_STRONG), tint)),
        );
        slider_with(
            ui,
            step,
            "Luminance",
            &mut grade[2],
            -1. ..=1.,
            0.,
            None,
            Some((palette.tone(Tone::BAR), palette.tone(Tone::SOFT))),
        );
    });
}

/// A wheel's color: hue 0–1 (red at the right, turning counter-clockwise as on
/// Lightroom's wheels) and saturation 0–1 (the centre to the rim).
#[derive(Clone, Copy, Debug, PartialEq)]
struct HueSat {
    hue: f32,
    saturation: f32,
}

impl HueSat {
    const NEUTRAL: HueSat = HueSat {
        hue: 0.,
        saturation: 0.,
    };
    fn of(grade: &[f32; 3]) -> Self {
        Self {
            hue: grade[0],
            saturation: grade[1],
        }
    }
    fn store(self, grade: &mut [f32; 3]) {
        grade[0] = self.hue;
        grade[1] = self.saturation;
    }
    /// The direction of this hue, a wheel radius long, y down as on screen.
    fn direction(hue: f32) -> Vec2 {
        let angle = hue * TAU;
        Vec2::new(angle.cos(), -angle.sin())
    }
    /// Where the puck sits, in wheel radii from the centre.
    fn offset(self) -> Vec2 {
        Self::direction(self.hue) * self.saturation
    }
    /// The color at `offset` (in radii, clamped to the rim), in whole degrees and
    /// percent as Lightroom keeps them. At the centre, where every hue looks the
    /// same, `hue` stays.
    fn at(offset: Vec2, hue: f32) -> Self {
        let hue = if offset.length() < 1e-4 {
            hue
        } else {
            round_hue((-offset.y).atan2(offset.x) / TAU)
        };
        Self {
            hue,
            saturation: round_saturation(offset.length()),
        }
    }
}
/// A hue in whole degrees, 0 to 359.
fn round_hue(hue: f32) -> f32 {
    (hue.rem_euclid(1.) * 360.).round() % 360. / 360.
}
fn round_saturation(saturation: f32) -> f32 {
    (saturation.clamp(0., 1.) * 100.).round() / 100.
}

/// How a drag moves the puck: with the pointer, or a tenth as far (Cmd/Ctrl).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Precision {
    Normal,
    Fine,
}
/// Whether a drag may change both hue and saturation, or only one (Shift).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Constraint {
    Free,
    HueOrSaturation,
}
/// What a constrained drag changes, picked by the way it first moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lock {
    Hue,
    Saturation,
}

/// A wheel drag in progress, kept in egui memory between frames.
#[derive(Clone, Copy, Debug)]
struct WheelDrag {
    /// The wheel's color when the drag began; History names what changed from it.
    start: HueSat,
    /// The puck as dragged, in radii from the centre, before any constraint.
    puck: Vec2,
    /// `puck` less the pointer, followed while the drag is not fine.
    grab: Vec2,
    lock: Option<Lock>,
}

impl WheelDrag {
    /// A drag from `pointer` (in radii from the centre): the puck jumps under the
    /// pointer, or is picked up where it is when `grab` says how far away it is.
    fn new(start: HueSat, pointer: Vec2, grab: Vec2) -> Self {
        Self {
            start,
            puck: pointer + grab,
            grab,
            lock: None,
        }
    }
    /// The color for the pointer now at `pointer`, having moved `delta` since the
    /// last update (both in radii).
    fn update(
        &mut self,
        pointer: Vec2,
        delta: Vec2,
        precision: Precision,
        constraint: Constraint,
    ) -> HueSat {
        match precision {
            Precision::Fine => {
                self.puck += delta * FINE;
                self.grab = self.puck - pointer;
            }
            Precision::Normal => self.puck = pointer + self.grab,
        }
        if constraint == Constraint::Free {
            self.lock = None;
            return HueSat::at(self.puck, self.start.hue);
        }
        let direction = HueSat::direction(self.start.hue);
        let moved = self.puck - self.start.offset();
        if self.lock.is_none() && moved.length() > LOCK_DISTANCE {
            let along = moved.dot(direction).abs();
            let across = moved.dot(direction.rot90()).abs();
            self.lock = Some(if along >= across {
                Lock::Saturation
            } else {
                Lock::Hue
            });
        }
        match self.lock {
            None => self.start,
            Some(Lock::Saturation) => HueSat {
                hue: self.start.hue,
                saturation: round_saturation(self.puck.dot(direction)),
            },
            Some(Lock::Hue) => HueSat {
                hue: HueSat::at(self.puck, self.start.hue).hue,
                saturation: self.start.saturation,
            },
        }
    }
}

/// A single click waits for egui's double-click window before changing the recipe.
/// This keeps a double-click reset to one edit, without an intermediate color in Undo.
#[derive(Clone, Copy)]
struct WheelClick {
    color: HueSat,
    before: [f32; 3],
    deadline: f64,
    frame: u64,
}

/// A hue/saturation wheel filling `rect`.
fn wheel(ui: &mut egui::Ui, step: &mut StepName, rect: Rect, grade: &mut [f32; 3], region: Region) {
    let id = ui.id().with(("grade-wheel", region));
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let center = rect.center();
    let radius = rect.width().min(rect.height()) / 2. - 3.;
    let to_offset = |p: Pos2| (p - center) / radius;
    let on_disc = |p: Pos2| p.distance(center) <= radius;
    // Keep the press origin through release (egui clears press_origin on release),
    // including a press and release delivered in the same frame.
    let press_memory = id.with("press-on-disc");
    let mut accepted = ui
        .data(|d| d.get_temp::<bool>(press_memory))
        .unwrap_or(false);
    ui.input(|i| {
        for event in &i.events {
            if let egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                ..
            } = event
            {
                accepted = on_disc(*pos);
            }
        }
    });
    let primary_down = ui.input(|i| i.pointer.primary_down());
    ui.data_mut(|d| {
        if primary_down {
            d.insert_temp(press_memory, accepted);
        } else {
            d.remove::<bool>(press_memory);
        }
    });
    let before = HueSat::of(grade);
    let modifiers = ui.input(|i| i.modifiers);
    let precision = if modifiers.command {
        Precision::Fine
    } else {
        Precision::Normal
    };
    let constraint = if modifiers.shift {
        Constraint::HueOrSaturation
    } else {
        Constraint::Free
    };
    let memory = id.with("drag");
    let mut drag: Option<WheelDrag> = ui.data(|d| d.get_temp(memory));
    let click_memory = id.with("click");
    let mut click: Option<WheelClick> = ui.data_mut(|d| {
        let pending = d.get_temp(click_memory);
        d.remove::<WheelClick>(click_memory);
        pending
    });
    let frame = ui.ctx().cumulative_frame_nr();
    let now = ui.input(|i| i.time);
    // A hidden panel or another edit must not replay a stale click later.
    click = click.filter(|c| c.frame + 1 >= frame && c.before == *grade);
    if ui.input(|i| {
        i.events.iter().any(|e| match e {
            egui::Event::PointerButton {
                pos, pressed: true, ..
            } => !on_disc(*pos),
            egui::Event::Key { pressed: true, .. } => true,
            _ => false,
        })
    }) {
        click = None;
    }
    if response.double_clicked() && accepted {
        click = None;
        HueSat::NEUTRAL.store(grade);
        drag = None;
    } else if let Some(pointer) = response.interact_pointer_pos() {
        let on_puck = |p: Pos2| (to_offset(p) - before.offset()).length() * radius <= PUCK_GRAB;
        let mut delta = response.drag_delta() / radius;
        // A plain click away from the puck moves it there, as in Lightroom.
        if response.clicked() && accepted && modifiers.is_none() && !on_puck(pointer) {
            click = Some(WheelClick {
                color: HueSat::at(to_offset(pointer), before.hue),
                before: *grade,
                deadline: now + ui.ctx().options(|o| o.input_options.max_double_click_delay),
                frame,
            });
        }
        if response.drag_started()
            && accepted
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        {
            click = None;
            // A plain press away from the puck moves it there too; a press on it, or
            // with Shift or Cmd held, keeps its color to start from.
            let grab = if on_puck(origin) || !modifiers.is_none() {
                before.offset() - to_offset(origin)
            } else {
                Vec2::ZERO
            };
            drag = Some(WheelDrag::new(before, to_offset(origin), grab));
            delta = to_offset(pointer) - to_offset(origin);
        }
        if response.dragged()
            && let Some(drag) = drag.as_mut()
        {
            drag.update(to_offset(pointer), delta, precision, constraint)
                .store(grade);
        }
    }
    if response.drag_stopped() {
        drag = None;
    }
    if let Some(mut pending) = click {
        if now >= pending.deadline && !ui.input(|i| i.pointer.primary_down()) {
            pending.color.store(grade);
        } else {
            pending.frame = frame;
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(
                    (pending.deadline - now).max(0.01),
                ));
            ui.data_mut(|d| d.insert_temp(click_memory, pending));
        }
    }
    let start = drag.map_or(before, |d| d.start);
    ui.data_mut(|d| match drag {
        Some(drag) => {
            d.insert_temp(memory, drag);
        }
        None => d.remove::<WheelDrag>(memory),
    });
    let after = HueSat::of(grade);
    if after != before {
        let (name, value) = wheel_step(region, start, after);
        step.name(name, value);
    }
    paint_disc(ui.painter(), center, radius, 72, 10);
    let painter = ui.painter();
    painter.circle_stroke(
        center,
        radius,
        Stroke::new(1., theme::palette(ui.ctx()).tone(Tone::BAR)),
    );
    painter.circle_filled(center, 1.5, Color32::from_gray(150));
    let puck = center + after.offset() * radius;
    let active = response.hovered() || response.dragged();
    painter.circle(
        puck,
        if active { 6. } else { 5. },
        hue_color(after.hue, after.saturation),
        Stroke::new(1.5, Color32::from_gray(240)),
    );
    response.on_hover_text(format!(
        "{}: drag to set hue and saturation · Shift for hue or saturation only · {} for fine \
         adjustment · double-click to reset",
        region.title(),
        if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Ctrl"
        },
    ));
}

/// History's name for a wheel edit from `start` to `end`: "Shadow Hue", "Shadow
/// Saturation", or both when the puck moved both ways.
fn wheel_step(region: Region, start: HueSat, end: HueSat) -> (String, String) {
    let hue = format!("{:.0}", end.hue * 360.);
    let saturation = format!("{:.0}", end.saturation * 100.);
    let step = region.step();
    match (start.hue != end.hue, start.saturation != end.saturation) {
        (true, false) => (format!("{step} Hue"), hue),
        (false, true) => (format!("{step} Saturation"), saturation),
        _ => (
            format!("{step} Hue & Saturation"),
            format!("{hue} / {saturation}"),
        ),
    }
}

/// A wheel's hue and saturation as typed or dragged numbers, centred in `rect`.
fn hue_saturation_fields(
    ui: &mut egui::Ui,
    step: &mut StepName,
    rect: Rect,
    grade: &mut [f32; 3],
    region: Region,
) {
    let size = Vec2::new(48., 18.);
    let gap = 6.;
    let left = Rect::from_min_size(
        Pos2::new(
            rect.center().x - gap / 2. - size.x,
            rect.center().y - size.y / 2.,
        ),
        size,
    );
    let right = left.translate(Vec2::new(size.x + gap, 0.));
    let mut degrees = (grade[0] * 360.).round();
    let mut percent = (grade[1] * 100.).round();
    let caption = region.step();
    let hue = ui
        .put(
            left,
            egui::DragValue::new(&mut degrees)
                .range(0. ..=360.)
                .speed(1.)
                .max_decimals(0)
                .prefix("H "),
        )
        .on_hover_text(format!("{caption} Hue"));
    let saturation = ui
        .put(
            right,
            egui::DragValue::new(&mut percent)
                .range(0. ..=100.)
                .speed(0.5)
                .max_decimals(0)
                .prefix("S "),
        )
        .on_hover_text(format!("{caption} Saturation"));
    if hue.changed() {
        grade[0] = degrees / 360.;
        step.name(format!("{caption} Hue"), format!("{degrees:.0}"));
    }
    if saturation.changed() {
        grade[1] = percent / 100.;
        step.name(format!("{caption} Saturation"), format!("{percent:.0}"));
    }
}

#[derive(Clone, Copy)]
struct LuminanceClick {
    value: f32,
    before: f32,
    deadline: f64,
    frame: u64,
}

/// A small wheel's Luminance: a dark-to-light rail with its number, −100 to 100.
fn luminance_rail(
    ui: &mut egui::Ui,
    step: &mut StepName,
    rect: Rect,
    value: &mut f32,
    region: Region,
) {
    let palette = theme::palette(ui.ctx());
    let field = Rect::from_min_size(
        Pos2::new(rect.right() - 40., rect.center().y - 9.),
        Vec2::new(40., 18.),
    );
    let area = Rect::from_min_max(rect.min, Pos2::new(field.left() - 4., rect.bottom()));
    let rail = Rect::from_min_max(
        Pos2::new(area.left() + 5., area.center().y - 1.),
        Pos2::new(area.right() - 5., area.center().y + 1.),
    );
    let before = *value;
    let id = ui.id().with(("grade-luminance", region));
    let response = ui
        .interact(area, id, Sense::click_and_drag())
        .on_hover_text(format!(
            "{} Luminance: drag · double-click to reset",
            region.title()
        ));
    // As with the wheel, wait out a possible double-click before applying a click.
    let memory = id.with("click");
    let mut click: Option<LuminanceClick> = ui.data_mut(|d| {
        let pending = d.get_temp(memory);
        d.remove::<LuminanceClick>(memory);
        pending
    });
    let frame = ui.ctx().cumulative_frame_nr();
    let now = ui.input(|i| i.time);
    click = click.filter(|c| c.frame + 1 >= frame && c.before == *value);
    if ui.input(|i| {
        i.events.iter().any(|e| match e {
            egui::Event::PointerButton {
                pos, pressed: true, ..
            } => !area.contains(*pos),
            egui::Event::Key { pressed: true, .. } => true,
            _ => false,
        })
    }) {
        click = None;
    }
    if response.double_clicked() {
        click = None;
        *value = 0.;
    } else if (response.dragged() || response.clicked())
        && let Some(p) = response.interact_pointer_pos()
    {
        let t = egui::remap_clamp(p.x, rail.left()..=rail.right(), 0. ..=1.);
        let target = ((t * 2. - 1.) * 100.).round() / 100.;
        if response.dragged() {
            click = None;
            *value = target;
        } else {
            click = Some(LuminanceClick {
                value: target,
                before,
                deadline: now + ui.ctx().options(|o| o.input_options.max_double_click_delay),
                frame,
            });
        }
    }
    if let Some(mut pending) = click {
        if now >= pending.deadline && !ui.input(|i| i.pointer.primary_down()) {
            *value = pending.value;
        } else {
            pending.frame = frame;
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(
                    (pending.deadline - now).max(0.01),
                ));
            ui.data_mut(|d| d.insert_temp(memory, pending));
        }
    }
    let mut percent = (*value * 100.).round();
    if ui
        .put(
            field,
            egui::DragValue::new(&mut percent)
                .range(-100. ..=100.)
                .speed(0.5)
                .max_decimals(0),
        )
        .changed()
    {
        *value = percent / 100.;
    }
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rail.left_top(), palette.tone(Tone::BAR));
    mesh.colored_vertex(rail.right_top(), palette.tone(Tone::SOFT));
    mesh.colored_vertex(rail.right_bottom(), palette.tone(Tone::SOFT));
    mesh.colored_vertex(rail.left_bottom(), palette.tone(Tone::BAR));
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    let painter = ui.painter();
    painter.add(mesh);
    let x = egui::lerp(rail.left()..=rail.right(), (*value + 1.) / 2.);
    painter.line_segment(
        [
            Pos2::new(rail.center().x, area.center().y - 4.),
            Pos2::new(rail.center().x, area.center().y + 4.),
        ],
        Stroke::new(1., palette.tone(Tone::DISABLED)),
    );
    let thumb = Pos2::new(x, area.center().y);
    painter.circle_filled(thumb, 3.5, palette.tone(Tone::SOFT));
    painter.circle_stroke(thumb, 3.5, Stroke::new(1., palette.tone(Tone::BAR)));
    if *value != before {
        step.name(
            format!("{} Luminance", region.step()),
            format!("{:.0}", *value * 100.),
        );
    }
}

/// The color a wheel shows at `hue` and `saturation`: grey at the centre, the hue
/// at the rim, muted as Lightroom's are.
fn hue_color(hue: f32, saturation: f32) -> Color32 {
    let rim = crate::develop::color::hue_rgb(hue).map(|v| (40. + v * 170.) as u8);
    Color32::from_gray(80).lerp_to_gamma(
        Color32::from_rgb(rim[0], rim[1], rim[2]),
        saturation.clamp(0., 1.),
    )
}

/// A disc of every hue and saturation: `sectors` round, `rings` out from the centre.
fn paint_disc(painter: &egui::Painter, center: Pos2, radius: f32, sectors: u32, rings: u32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, hue_color(0., 0.));
    for ring in 1..=rings {
        let saturation = ring as f32 / rings as f32;
        for sector in 0..sectors {
            let hue = sector as f32 / sectors as f32;
            mesh.colored_vertex(
                center + HueSat::direction(hue) * radius * saturation,
                hue_color(hue, saturation),
            );
        }
    }
    let at = |ring: u32, sector: u32| 1 + (ring - 1) * sectors + sector % sectors;
    for sector in 0..sectors {
        mesh.add_triangle(0, at(1, sector), at(1, sector + 1));
        for ring in 2..=rings {
            let (a, b) = (at(ring - 1, sector), at(ring - 1, sector + 1));
            let (c, d) = (at(ring, sector), at(ring, sector + 1));
            mesh.add_triangle(a, c, d);
            mesh.add_triangle(a, d, b);
        }
    }
    painter.add(mesh);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_puck_puts_red_at_the_right_and_turns_counter_clockwise() {
        let red = HueSat {
            hue: 0.,
            saturation: 1.,
        };
        assert!((red.offset() - Vec2::new(1., 0.)).length() < 1e-6);
        // 90° (yellow-green) is straight up: y points down on screen.
        let up = HueSat::at(Vec2::new(0., -0.5), 0.);
        assert_eq!(
            up,
            HueSat {
                hue: 0.25,
                saturation: 0.5
            }
        );
        // Whole degrees and percent, as Lightroom stores them, and the rim at most.
        let blue = HueSat::at(HueSat::direction(210.4 / 360.) * 1.7, 0.);
        assert_eq!(
            blue,
            HueSat {
                hue: 210. / 360.,
                saturation: 1.
            }
        );
        // At the centre the hue stays.
        assert_eq!(HueSat::at(Vec2::ZERO, 0.6).hue, 0.6);
    }

    #[test]
    fn shift_keeps_a_drag_to_saturation_or_hue_and_cmd_moves_it_finely() {
        let start = HueSat {
            hue: 0.,
            saturation: 0.5,
        };
        // Outwards along red: saturation only, however the pointer wanders after.
        let mut drag = WheelDrag::new(start, start.offset(), Vec2::ZERO);
        let pointer = Vec2::new(0.8, 0.);
        let end = drag.update(
            pointer,
            Vec2::new(0.3, 0.),
            Precision::Normal,
            Constraint::HueOrSaturation,
        );
        assert_eq!(
            end,
            HueSat {
                hue: 0.,
                saturation: 0.8
            }
        );
        let end = drag.update(
            Vec2::new(0.8, -0.3),
            Vec2::new(0., -0.3),
            Precision::Normal,
            Constraint::HueOrSaturation,
        );
        assert_eq!((end.hue, end.saturation), (0., 0.8));
        // Across the radius: hue only.
        let mut drag = WheelDrag::new(start, start.offset(), Vec2::ZERO);
        let end = drag.update(
            Vec2::new(0.5, -0.2),
            Vec2::new(0., -0.2),
            Precision::Normal,
            Constraint::HueOrSaturation,
        );
        assert_eq!(end.saturation, 0.5);
        assert!(end.hue > 0.05 && end.hue < 0.07, "{end:?}");
        // Fine: a tenth of the pointer's movement.
        let mut drag = WheelDrag::new(start, start.offset(), Vec2::ZERO);
        let end = drag.update(
            Vec2::new(0.8, 0.),
            Vec2::new(0.3, 0.),
            Precision::Fine,
            Constraint::Free,
        );
        assert_eq!(
            end,
            HueSat {
                hue: 0.,
                saturation: 0.53
            }
        );
    }

    /// Runs frames of `wheel` in a 200-point square at the window's corner.
    struct Harness {
        ctx: egui::Context,
        time: f64,
        rect: Rect,
        luminance: bool,
        step: StepName,
    }
    impl Harness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            let margin = ctx.global_style().spacing.window_margin;
            let corner = Pos2::new(margin.left as f32, margin.top as f32);
            Self {
                ctx,
                time: 0.,
                luminance: false,
                step: StepName::default(),
                rect: Rect::from_min_size(corner + Vec2::splat(10.), Vec2::splat(200.)),
            }
        }
        fn frame(
            &mut self,
            grade: &mut [f32; 3],
            mut events: Vec<egui::Event>,
            modifiers: egui::Modifiers,
        ) {
            self.time += 0.5;
            events.insert(0, egui::Event::ModifiersChanged(modifiers));
            let rect = self.rect;
            let mut output = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(400.))),
                    time: Some(self.time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    if self.luminance {
                        luminance_rail(ui, &mut self.step, rect, &mut grade[2], Region::Shadows);
                    } else {
                        wheel(ui, &mut self.step, rect, grade, Region::Shadows);
                    }
                },
            );
            output.textures_delta.clear();
        }
        fn at(&self, offset: Vec2) -> Pos2 {
            self.rect.center() + offset * (self.rect.width() / 2. - 3.)
        }
        fn drag(
            &mut self,
            grade: &mut [f32; 3],
            path: &[Vec2],
            modifiers: egui::Modifiers,
        ) -> Option<(String, String)> {
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers,
            };
            let first = self.at(path[0]);
            self.frame(grade, vec![egui::Event::PointerMoved(first)], modifiers);
            self.frame(grade, vec![button(first, true)], modifiers);
            for p in &path[1..] {
                let p = self.at(*p);
                self.frame(grade, vec![egui::Event::PointerMoved(p)], modifiers);
            }
            let step = self.step.given();
            let last = self.at(*path.last().unwrap());
            self.frame(grade, vec![button(last, false)], modifiers);
            step
        }
    }

    #[test]
    fn dragging_a_wheel_sets_hue_and_saturation_and_names_the_step() {
        let mut h = Harness::new();
        let mut grade = [0., 0., 0.25];
        // A press away from the puck moves it there, then it follows the pointer.
        let target = HueSat::direction(210. / 360.) * 0.4;
        let step = h.drag(
            &mut grade,
            &[Vec2::new(0., -0.5), target * 0.5, target],
            egui::Modifiers::NONE,
        );
        assert_eq!(grade, [210. / 360., 0.4, 0.25]);
        assert_eq!(
            step,
            Some(("Shadow Hue & Saturation".into(), "210 / 40".into()))
        );
        // Shift outwards from the puck: saturation alone, named so.
        let step = h.drag(
            &mut grade,
            &[target, target * 1.2, target * 1.5 + Vec2::new(0.05, 0.)],
            egui::Modifiers::SHIFT,
        );
        assert_eq!(grade[0], 210. / 360.);
        assert!(grade[1] > 0.55 && grade[1] < 0.65, "{grade:?}");
        assert_eq!(step.map(|s| s.0), Some("Shadow Saturation".into()));
        // A plain click moves the puck under the pointer.
        let left = h.at(Vec2::new(-0.3, 0.));
        let press = |pressed| egui::Event::PointerButton {
            pos: left,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        h.frame(
            &mut grade,
            vec![egui::Event::PointerMoved(left)],
            egui::Modifiers::NONE,
        );
        h.frame(&mut grade, vec![press(true)], egui::Modifiers::NONE);
        h.time -= 0.45;
        h.frame(&mut grade, vec![press(false)], egui::Modifiers::NONE);
        h.frame(&mut grade, vec![], egui::Modifiers::NONE);
        assert_eq!((grade[0], grade[1]), (0.5, 0.3));
        // Double-click resets hue and saturation, not Luminance.
        let at = h.at(Vec2::ZERO);
        let click = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        h.frame(
            &mut grade,
            vec![egui::Event::PointerMoved(at)],
            egui::Modifiers::NONE,
        );
        for pressed in [true, false, true, false] {
            h.time -= 0.45;
            h.frame(&mut grade, vec![click(pressed)], egui::Modifiers::NONE);
        }
        assert_eq!(grade, [0., 0., 0.25]);
    }

    #[test]
    fn empty_corners_ignore_clicks_double_clicks_and_drags() {
        for diameter in [SMALL_WHEEL, LARGE_WHEEL] {
            for corner in [
                Vec2::new(-0.9, -0.9),
                Vec2::new(0.9, -0.9),
                Vec2::new(-0.9, 0.9),
                Vec2::new(0.9, 0.9),
            ] {
                let mut h = Harness::new();
                h.rect = Rect::from_min_size(h.rect.min, Vec2::splat(diameter));
                let original = [0.25, 0.4, 0.2];
                let mut grade = original;
                let at = h.at(corner);
                h.frame(
                    &mut grade,
                    vec![egui::Event::PointerMoved(at)],
                    egui::Modifiers::NONE,
                );
                for pressed in [true, false, true, false] {
                    h.time -= 0.45;
                    h.frame(
                        &mut grade,
                        vec![egui::Event::PointerButton {
                            pos: at,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        }],
                        egui::Modifiers::NONE,
                    );
                    assert_eq!(grade, original);
                }
                h.frame(&mut grade, vec![], egui::Modifiers::NONE);
                assert_eq!(grade, original);
                h.drag(
                    &mut grade,
                    &[corner, Vec2::ZERO, Vec2::new(0.5, 0.)],
                    egui::Modifiers::NONE,
                );
                assert_eq!(grade, original);
                // A drag that starts on the disc can still move beyond its rim.
                h.drag(
                    &mut grade,
                    &[Vec2::ZERO, Vec2::new(1.5, 0.)],
                    egui::Modifiers::NONE,
                );
                assert_eq!(grade, [0., 1., 0.2]);
            }
        }
    }

    #[test]
    fn double_click_reset_undo_restores_the_grade_before_either_click() {
        let mut h = Harness::new();
        let mut history = crate::app::history::History::default();
        let mut recipe = Recipe::default();
        recipe.grading[0] = [0.25, 0.4, 0.2];
        let original = recipe.clone();
        let at = h.at(Vec2::new(-0.5, 0.));
        h.frame(
            &mut recipe.grading[0],
            vec![egui::Event::PointerMoved(at)],
            egui::Modifiers::NONE,
        );
        for pressed in [true, false, true, false] {
            history.begin_frame();
            let before = recipe.clone();
            h.time -= 0.45;
            h.frame(
                &mut recipe.grading[0],
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
                egui::Modifiers::NONE,
            );
            history.observe(before, &recipe, pressed);
        }
        assert_eq!(recipe.grading[0], [0., 0., 0.2]);
        assert_eq!(history.steps().1, 1);
        assert!(history.undo(&mut recipe));
        assert_eq!(recipe, original);
        // No delayed first click may reapply itself after the reset.
        h.frame(&mut recipe.grading[0], vec![], egui::Modifiers::NONE);
        assert_eq!(recipe, original);
    }

    #[test]
    fn luminance_double_click_is_one_undoable_reset() {
        let mut h = Harness::new();
        h.luminance = true;
        let mut history = crate::app::history::History::default();
        let mut recipe = Recipe::default();
        recipe.grading[0] = [0.25, 0.4, -0.3];
        let original = recipe.clone();
        let at = Pos2::new(h.rect.left() + 120., h.rect.center().y);
        h.frame(
            &mut recipe.grading[0],
            vec![egui::Event::PointerMoved(at)],
            egui::Modifiers::NONE,
        );
        for pressed in [true, false, true, false] {
            history.begin_frame();
            let before = recipe.clone();
            h.time -= 0.45;
            h.frame(
                &mut recipe.grading[0],
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
                egui::Modifiers::NONE,
            );
            if let Some(step) = h.step.take() {
                history.label(step);
            }
            history.observe(before, &recipe, pressed);
        }
        assert_eq!(recipe.grading[0], [0.25, 0.4, 0.]);
        assert_eq!(history.steps().1, 1);
        assert_eq!(history.steps().0[0].name, "Shadow Luminance");
        assert!(history.undo(&mut recipe));
        assert_eq!(recipe, original);
        h.frame(&mut recipe.grading[0], vec![], egui::Modifiers::NONE);
        assert_eq!(recipe, original);
    }

    #[test]
    fn luminance_single_click_and_drag_still_set_the_value() {
        let mut h = Harness::new();
        h.luminance = true;
        let mut grade = [0.25, 0.4, -0.3];
        let at = Pos2::new(h.rect.left() + 120., h.rect.center().y);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        h.frame(
            &mut grade,
            vec![egui::Event::PointerMoved(at)],
            egui::Modifiers::NONE,
        );
        h.frame(&mut grade, vec![button(at, true)], egui::Modifiers::NONE);
        h.time -= 0.45;
        h.frame(&mut grade, vec![button(at, false)], egui::Modifiers::NONE);
        h.frame(&mut grade, vec![], egui::Modifiers::NONE);
        assert_eq!(grade, [0.25, 0.4, 0.58]);
        // Dragging remains immediate, even outside the rail's ends.
        h.frame(&mut grade, vec![button(at, true)], egui::Modifiers::NONE);
        let left = Pos2::new(h.rect.left(), at.y);
        h.frame(
            &mut grade,
            vec![egui::Event::PointerMoved(left)],
            egui::Modifiers::NONE,
        );
        assert_eq!(grade, [0.25, 0.4, -1.]);
        h.frame(&mut grade, vec![button(left, false)], egui::Modifiers::NONE);
    }

    #[test]
    fn grading_from_the_wheel_renders_as_the_same_numbers_typed() {
        // The wheel stores into the recipe's existing fields: a grade set by dragging
        // equals one set with the sliders, so the pipeline sees the same recipe.
        let mut h = Harness::new();
        let mut grade = [0.; 3];
        h.drag(
            &mut grade,
            &[Vec2::ZERO, HueSat::direction(40. / 360.) * 0.25],
            egui::Modifiers::NONE,
        );
        let mut dragged = Recipe::default();
        *Region::Shadows.grade(&mut dragged) = grade;
        let mut typed = Recipe::default();
        typed.grading[0] = [40. / 360., 0.25, 0.];
        assert_eq!(dragged, typed);
    }
}
