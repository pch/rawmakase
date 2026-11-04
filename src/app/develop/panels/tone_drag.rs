//! Dragging in the histogram, as in Lightroom: the histogram is split into
//! Blacks, Shadows, Exposure, Highlights and Whites, and dragging sideways in
//! one moves that slider. The regions are fifths of the width, along the
//! histogram's grid lines.
use crate::app::develop::step_name::StepName;
use crate::model::recipe::Recipe;
use eframe::egui::{self, CursorIcon, Rect, Sense};

/// A histogram region and the Basic slider it drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum ToneRegion {
    Blacks,
    Shadows,
    Exposure,
    Highlights,
    Whites,
}
impl ToneRegion {
    pub(in crate::app) const ALL: [Self; 5] = [
        Self::Blacks,
        Self::Shadows,
        Self::Exposure,
        Self::Highlights,
        Self::Whites,
    ];
    /// The region at `x`, a fraction of the histogram's width.
    pub(in crate::app) fn at(x: f32) -> Self {
        Self::ALL[((x.clamp(0., 1.) * 5.) as usize).min(4)]
    }
    /// Left and right edges as fractions of the width.
    pub(in crate::app) fn span(self) -> [f32; 2] {
        let i = Self::ALL.iter().position(|r| *r == self).unwrap_or(0) as f32;
        [i / 5., (i + 1.) / 5.]
    }
    pub(in crate::app) fn label(self) -> &'static str {
        match self {
            Self::Blacks => "Blacks",
            Self::Shadows => "Shadows",
            Self::Exposure => "Exposure",
            Self::Highlights => "Highlights",
            Self::Whites => "Whites",
        }
    }
    pub(in crate::app) fn value(self, r: &Recipe) -> f32 {
        match self {
            Self::Blacks => r.blacks,
            Self::Shadows => r.shadows,
            Self::Exposure => r.exposure,
            Self::Highlights => r.highlights,
            Self::Whites => r.whites,
        }
    }
    fn value_mut(self, r: &mut Recipe) -> &mut f32 {
        match self {
            Self::Blacks => &mut r.blacks,
            Self::Shadows => &mut r.shadows,
            Self::Exposure => &mut r.exposure,
            Self::Highlights => &mut r.highlights,
            Self::Whites => &mut r.whites,
        }
    }
    /// The slider's range: Exposure ±5 EV, the others ±100 (stored ±1).
    fn limit(self) -> f32 {
        if self == Self::Exposure { 5. } else { 1. }
    }
    /// The value as its slider shows it: "+0.35" EV, "+12".
    pub(in crate::app) fn display(self, value: f32) -> String {
        let text = if self == Self::Exposure {
            format!("{value:.2}")
        } else {
            format!("{:.0}", value * 100.)
        };
        if text.trim_start_matches(['-', '0', '.']).is_empty() {
            text.trim_start_matches('-').to_string()
        } else if value > 0. {
            format!("+{text}")
        } else {
            text
        }
    }
    /// `start` moved by a drag of `dx`, a fraction of the histogram's width:
    /// the whole width moves the slider from its centre to one end. Steps as
    /// the slider's drag does: 0.05 EV, or one unit. A value outside the
    /// slider's range (an imported Exposure of +7 EV) stays until the drag
    /// moves it back in, and one that does not reach the next step is kept.
    pub(in crate::app) fn dragged(self, start: f32, dx: f32) -> f32 {
        let limit = self.limit();
        let steps = if self == Self::Exposure { 20. } else { 100. };
        let round = |v: f32| (v * steps).round() / steps;
        let value = round(start + dx * limit);
        if value == round(start) {
            return start;
        }
        value.clamp((-limit).min(start), limit.max(start))
    }
}

/// A drag in progress: its region and the value when it started.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct ToneDrag {
    region: ToneRegion,
    start: f32,
}

/// The histogram's drag handling over `rect`, except over `controls` (the
/// clipping triangles), which keep their presses. Primary button only.
/// Returns the region to show: the one dragged, or the one under the pointer.
pub(in crate::app) fn tone_drag_ui(
    ui: &egui::Ui,
    step: &mut StepName,
    rect: Rect,
    controls: &[Rect],
    drag: &mut Option<ToneDrag>,
    recipe: &mut Recipe,
) -> Option<ToneRegion> {
    let primary = egui::PointerButton::Primary;
    let response = ui.interact(rect, ui.id().with("histogram-drag"), Sense::drag());
    let fraction = |x: f32| (x - rect.left()) / rect.width().max(1.);
    let free = |p: &egui::Pos2| !controls.iter().any(|c| c.contains(*p));
    // Where the press was, not where the drag was noticed.
    let pressed_at = ui.input(|i| i.pointer.press_origin());
    if response.drag_started_by(primary)
        && let Some(p) = pressed_at.filter(free)
    {
        let region = ToneRegion::at(fraction(p.x));
        *drag = Some(ToneDrag {
            region,
            start: region.value(recipe),
        });
    }
    if let Some(d) = *drag {
        let dragged = response.dragged_by(primary);
        if dragged {
            let dx = response.total_drag_delta().map_or(0., |v| v.x) / rect.width().max(1.);
            let value = d.region.dragged(d.start, dx);
            let slider = d.region.value_mut(recipe);
            // Named only when it changed, or the name would go to the next edit.
            if *slider != value {
                *slider = value;
                step.name(d.region.label().to_string(), d.region.display(value));
            }
        } else {
            *drag = None;
        }
    }
    let shown = drag.map(|d| d.region).or_else(|| {
        response
            .hover_pos()
            .filter(free)
            .map(|p| ToneRegion::at(fraction(p.x)))
    });
    if shown.is_some() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::history::History;
    use eframe::egui::{Pos2, Vec2};

    #[test]
    fn the_histogram_splits_into_fifths_from_blacks_to_whites() {
        assert_eq!(ToneRegion::at(0.), ToneRegion::Blacks);
        assert_eq!(ToneRegion::at(0.19), ToneRegion::Blacks);
        assert_eq!(ToneRegion::at(0.21), ToneRegion::Shadows);
        assert_eq!(ToneRegion::at(0.5), ToneRegion::Exposure);
        assert_eq!(ToneRegion::at(0.7), ToneRegion::Highlights);
        assert_eq!(ToneRegion::at(1.), ToneRegion::Whites);
        assert_eq!(ToneRegion::at(-3.), ToneRegion::Blacks);
        assert_eq!(ToneRegion::Highlights.span(), [0.6, 0.8]);
    }

    #[test]
    fn a_drag_moves_the_slider_in_its_own_steps_within_range() {
        // The whole width moves from the centre to one end.
        assert_eq!(ToneRegion::Exposure.dragged(0., 1.), 5.);
        assert_eq!(ToneRegion::Shadows.dragged(0., -0.25), -0.25);
        assert_eq!(ToneRegion::Exposure.dragged(1., 0.013), 1.05);
        assert_eq!(ToneRegion::Whites.dragged(0.9, 0.5), 1.);
        assert_eq!(ToneRegion::Exposure.display(0.35), "+0.35");
        assert_eq!(ToneRegion::Blacks.display(-0.12), "-12");
        assert_eq!(ToneRegion::Blacks.display(-0.001), "0");
    }

    #[test]
    fn a_value_is_kept_until_the_drag_reaches_another_step() {
        // An imported +7 EV, beyond the slider: kept, then moved back in.
        assert_eq!(ToneRegion::Exposure.dragged(7., 0.), 7.);
        assert_eq!(ToneRegion::Exposure.dragged(7., -0.002), 7.);
        assert_eq!(ToneRegion::Exposure.dragged(7., -0.2), 6.);
        assert_eq!(ToneRegion::Exposure.dragged(7., 0.2), 7.);
        // A typed value between steps does not snap on the press.
        assert_eq!(ToneRegion::Shadows.dragged(0.333, 0.001), 0.333);
        assert_eq!(ToneRegion::Shadows.dragged(0.333, 0.01), 0.34);
    }

    /// The histogram at (0, 0)–(500, 100), drawn as the editor draws it, with
    /// History observing each frame.
    struct Harness {
        ctx: egui::Context,
        recipe: Recipe,
        drag: Option<ToneDrag>,
        history: History,
        controls: Vec<Rect>,
        step: StepName,
    }
    impl Harness {
        const RECT: Rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(500., 100.));
        fn new(controls: Vec<Rect>) -> Self {
            let mut h = Self {
                ctx: egui::Context::default(),
                recipe: Recipe::default(),
                drag: None,
                history: History::default(),
                controls,
                step: StepName::default(),
            };
            h.frame(vec![]);
            h
        }
        fn frame(&mut self, events: Vec<egui::Event>) -> Option<ToneRegion> {
            let before = self.recipe.clone();
            let mut shown = None;
            let (recipe, drag, controls, step) = (
                &mut self.recipe,
                &mut self.drag,
                &self.controls,
                &mut self.step,
            );
            let mut output = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(600.))),
                    events,
                    ..Default::default()
                },
                |ui| shown = tone_drag_ui(ui, step, Self::RECT, controls, drag, recipe),
            );
            output.textures_delta.clear();
            if let Some(step) = self.step.take() {
                self.history.label(step);
            }
            let down = self.ctx.input(|i| i.pointer.primary_down());
            self.history.observe(before, &self.recipe, down);
            shown
        }
        /// Presses `button` at `from`, moves through `to` and releases.
        fn drag(&mut self, button: egui::PointerButton, from: Pos2, to: &[f32]) {
            let press = |pos, pressed| egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            self.frame(vec![egui::Event::PointerMoved(from), press(from, true)]);
            let mut at = from;
            for x in to {
                at = Pos2::new(*x, from.y);
                self.frame(vec![egui::Event::PointerMoved(at)]);
            }
            self.frame(vec![press(at, false)]);
        }
        fn steps(&self) -> Vec<(String, String)> {
            let (steps, _) = self.history.steps();
            steps
                .iter()
                .map(|s| (s.name.clone(), s.value.clone()))
                .collect()
        }
    }

    #[test]
    fn one_drag_is_one_history_step_named_after_its_region() {
        let mut h = Harness::new(Vec::new());
        // Hovering shows the region without changing anything.
        let start = Pos2::new(70., 50.);
        assert_eq!(
            h.frame(vec![egui::Event::PointerMoved(start)]),
            Some(ToneRegion::Blacks)
        );
        // Leaving the region mid-drag keeps driving Blacks.
        h.drag(egui::PointerButton::Primary, start, &[120., 170., 220.]);
        assert_eq!(h.recipe.blacks, 0.3);
        assert_eq!(h.recipe.exposure, 0.);
        assert_eq!(h.steps(), [("Blacks".into(), "+30".into())]);
    }

    #[test]
    fn only_a_primary_drag_away_from_the_triangles_edits() {
        let triangle = Rect::from_min_size(Pos2::new(480., 0.), Vec2::splat(16.));
        let mut h = Harness::new(vec![triangle]);
        h.drag(
            egui::PointerButton::Primary,
            Pos2::new(488., 8.),
            &[440., 400.],
        );
        h.drag(
            egui::PointerButton::Secondary,
            Pos2::new(250., 50.),
            &[300., 350.],
        );
        // A press without a move names no step either.
        h.drag(egui::PointerButton::Primary, Pos2::new(250., 50.), &[]);
        assert_eq!(h.recipe, Recipe::default());
        assert!(h.steps().is_empty());
        assert!(h.step.given().is_none());
    }

    #[test]
    fn a_drag_does_not_carry_over_to_the_next_photo() {
        let mut h = Harness::new(Vec::new());
        let press = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let start = Pos2::new(70., 50.);
        h.frame(vec![egui::Event::PointerMoved(start), press(start, true)]);
        h.frame(vec![egui::Event::PointerMoved(Pos2::new(120., 50.))]);
        assert_eq!(h.recipe.blacks, 0.1);
        // Another photo opens mid-drag, as Left/Right does.
        let mut view = crate::app::develop::state::ViewState {
            tone_drag: h.drag,
            ..Default::default()
        };
        view.clear_document();
        assert_eq!(view.tone_drag, None);
        h.drag = view.tone_drag;
        h.recipe = Recipe::default();
        h.frame(vec![egui::Event::PointerMoved(Pos2::new(220., 50.))]);
        h.frame(vec![press(Pos2::new(220., 50.), false)]);
        assert_eq!(h.recipe, Recipe::default());
    }
}
