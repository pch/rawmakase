//! The Transform panel's Guided Upright tool (Shift+T), as in Lightroom: up to four
//! guides drawn along edges that should be vertical or horizontal, each end movable,
//! with a loupe while an end is placed and an optional grid. Every gesture is one
//! History step, and the correction is solved again after it (docs/transform.md).
use super::{Editor, state::Tool};
use crate::develop::{Geometry, guided};
use crate::model::transform::{UprightGuide, UprightMode};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};

/// Shortest guide drawn on screen, in points: a click or a slip is not a guide.
const MIN_DRAWN: f32 = 10.;
/// How near, in points, the pointer must be to a guide's end to move it.
const END_REACH: f32 = 9.;
/// How near, in points, a click must be to a guide to select it.
const LINE_REACH: f32 = 5.;

/// Which end of a guide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum End {
    A,
    B,
}

/// A guide being drawn or one of its ends moved, in screen points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum GuideDrag {
    New { from: Pos2, to: Pos2 },
    End { guide: usize, end: End, to: Pos2 },
}

/// The Guided tool's view state: what is selected and dragged, and its view options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct GuidedTool {
    pub(super) selected: Option<usize>,
    pub(super) drag: Option<GuideDrag>,
    /// Lightroom's Show Loupe: a magnified view while an end is placed.
    pub(super) loupe: bool,
    /// A grid over the photo to judge the correction by.
    pub(super) grid: bool,
}
impl Default for GuidedTool {
    fn default() -> Self {
        Self {
            selected: None,
            drag: None,
            loupe: true,
            grid: false,
        }
    }
}
impl GuidedTool {
    pub(super) fn clear_document(&mut self) {
        self.selected = None;
        self.drag = None;
    }
}

/// What a press on the photo hits.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    End(usize, End),
    Line(usize),
}

/// The guides with `drag` applied, once it is released: a new guide added, or an end
/// moved; None when nothing changes (a new guide too short to count).
fn released(
    guides: &[UprightGuide],
    drag: GuideDrag,
    frame: impl Fn(Pos2) -> [f32; 2],
) -> Option<Vec<UprightGuide>> {
    let mut out = guides.to_vec();
    match drag {
        GuideDrag::New { from, to } => {
            if from.distance(to) < MIN_DRAWN || out.len() >= crate::model::transform::MAX_GUIDES {
                return None;
            }
            out.push(UprightGuide {
                a: frame(from),
                b: frame(to),
            });
        }
        GuideDrag::End { guide, end, to } => {
            let g = out.get_mut(guide)?;
            let p = frame(to);
            match end {
                End::A => g.a = p,
                End::B => g.b = p,
            }
        }
    }
    Some(out)
}

impl Editor {
    /// Opens or closes the Guided tool (Shift+T). Opening it chooses Guided Upright,
    /// as in Lightroom.
    pub(super) fn toggle_guided_tool(&mut self) {
        self.view.toggle(Tool::Guided);
        if self.view.is(Tool::Guided) {
            self.choose_guided();
        }
    }
    /// Chooses Guided Upright, analysing the photo first if it hasn't been, so the
    /// guides drawn have their correction at once.
    pub(super) fn choose_guided(&mut self) {
        if self.document.edit.recipe().upright.mode != UprightMode::Guided {
            self.document.edit.recipe_mut().upright.mode = UprightMode::Guided;
            self.document
                .edit
                .name_next_step(super::history::Step::new("Upright", "Guided"));
        }
        let u = &self.document.edit.recipe().upright;
        if u.guides.is_empty() {
            self.status = guided::Issue::TooFew.message().into();
        }
        if u.corrections.len() < UprightMode::Guided.code() && !self.document.upright.is_running() {
            self.start_upright();
        }
    }
    /// Sets the guides as one History step and solves them; says in the status line
    /// when they correct less than they might.
    pub(super) fn set_guides(&mut self, guides: Vec<UprightGuide>, step: &str) {
        let im = self.document.full().cloned();
        let r = self.document.edit.recipe_mut();
        r.upright.mode = UprightMode::Guided;
        r.upright.guides = guides;
        let count = r.upright.guides.len();
        let issue = match im {
            Some(im) if r.upright.corrections.len() >= UprightMode::Guided.code() => {
                guided::store(r, &im.metadata)
            }
            // Solved when the analysis lands.
            _ => {
                self.ensure_upright();
                None
            }
        };
        let issue = issue.or((count < 2).then_some(guided::Issue::TooFew));
        match issue {
            Some(issue) => self.status = issue.message().into(),
            None if self.status.starts_with("Guided Upright") => self.status.clear(),
            None => {}
        }
        let value = match count {
            1 => "1 guide".to_string(),
            n => format!("{n} guides"),
        };
        self.document
            .edit
            .name_next_step(super::history::Step::new(step, value));
    }
    /// Deletes the selected guide (Delete).
    pub(super) fn delete_guide(&mut self) {
        let Some(i) = self.view.guided.selected.take() else {
            return;
        };
        let mut guides = self.document.edit.recipe().upright.guides.clone();
        if i < guides.len() {
            guides.remove(i);
            self.set_guides(guides, "Delete Guide");
        }
    }
    /// The Guided tool's keys: Delete and Backspace delete the selected guide.
    pub(super) fn guided_keys(&mut self, i: &egui::InputState) {
        if !i.modifiers.command
            && !i.modifiers.alt
            && (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
        {
            self.delete_guide();
        }
    }

    /// Draws the guides over the photo shown at `rect` and handles drawing, moving and
    /// selecting them.
    pub(super) fn guided_overlay(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        rect: Rect,
        area: Rect,
    ) {
        let Some(im) = self.document.full().cloned() else {
            return;
        };
        let g = Geometry::new(&im, &self.effective_recipe(), 0);
        let to_screen = |p: [f32; 2]| {
            let [u, v] = g.from_upright_frame(p);
            rect.min + Vec2::new(u, v) * rect.size()
        };
        let to_frame = |p: Pos2| {
            let q = (p - rect.min) / rect.size();
            g.upright_frame(q.x, q.y)
        };
        let guides = self.document.edit.recipe().upright.guides.clone();
        let ends: Vec<[Pos2; 2]> = guides
            .iter()
            .map(|g| [to_screen(g.a), to_screen(g.b)])
            .collect();
        let hit = |p: Pos2| -> Option<Hit> {
            let end = ends.iter().enumerate().rev().find_map(|(i, [a, b])| {
                if a.distance(p) < END_REACH {
                    Some(Hit::End(i, End::A))
                } else if b.distance(p) < END_REACH {
                    Some(Hit::End(i, End::B))
                } else {
                    None
                }
            });
            end.or_else(|| {
                ends.iter()
                    .enumerate()
                    .rev()
                    .find(|(_, [a, b])| distance_to_segment(p, *a, *b) < LINE_REACH)
                    .map(|(i, _)| Hit::Line(i))
            })
        };
        let tool = &mut self.view.guided;
        tool.selected = tool.selected.filter(|i| *i < guides.len());
        if let Some(p) = response.hover_pos().filter(|p| area.contains(*p)) {
            ui.ctx().set_cursor_icon(match hit(p) {
                Some(Hit::End(..)) => egui::CursorIcon::Grab,
                Some(Hit::Line(_)) => egui::CursorIcon::PointingHand,
                None => egui::CursorIcon::Crosshair,
            });
        }
        if response.drag_started()
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        {
            tool.drag = match hit(origin) {
                Some(Hit::End(guide, end)) => {
                    tool.selected = Some(guide);
                    Some(GuideDrag::End {
                        guide,
                        end,
                        to: origin,
                    })
                }
                _ if guides.len() >= crate::model::transform::MAX_GUIDES => {
                    self.status =
                        "Guided Upright takes four guides; delete one to draw another".into();
                    None
                }
                _ if rect.contains(origin) => Some(GuideDrag::New {
                    from: origin,
                    to: origin,
                }),
                _ => None,
            };
        }
        let tool = &mut self.view.guided;
        if let (Some(drag), Some(p)) = (&mut tool.drag, response.interact_pointer_pos()) {
            // Ends stay on the photo.
            let p = p.clamp(rect.min, rect.max);
            match drag {
                GuideDrag::New { to, .. } | GuideDrag::End { to, .. } => *to = p,
            }
        }
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            tool.selected = match hit(p) {
                Some(Hit::End(i, _) | Hit::Line(i)) => Some(i),
                None => None,
            };
        }
        let drag = tool.drag;
        let selected = tool.selected;
        let (loupe, grid) = (tool.loupe, tool.grid);
        let painter = ui.painter().with_clip_rect(area);
        if grid {
            grid_lines(&painter, rect.intersect(area));
        }
        for (i, [a, b]) in ends.iter().enumerate() {
            let (a, b) = match drag {
                Some(GuideDrag::End { guide, end, to }) if guide == i => match end {
                    End::A => (to, *b),
                    End::B => (*a, to),
                },
                _ => (*a, *b),
            };
            guide_line(&painter, a, b, selected == Some(i));
        }
        if let Some(GuideDrag::New { from, to }) = drag {
            guide_line(&painter, from, to, true);
        }
        let placing = match drag {
            Some(GuideDrag::New { to, .. } | GuideDrag::End { to, .. }) => Some(to),
            None => None,
        };
        if loupe
            && let Some(at) = placing
            && let Some(texture) = self.preview.texture.as_ref().map(|t| t.id())
        {
            magnifier(ui, texture, rect, at, area);
        }
        if response.drag_stopped()
            && let Some(drag) = self.view.guided.drag.take()
        {
            let step = match drag {
                GuideDrag::New { .. } => "Add Guide",
                GuideDrag::End { .. } => "Move Guide",
            };
            if let Some(next) = released(&guides, drag, to_frame) {
                if matches!(drag, GuideDrag::New { .. }) {
                    self.view.guided.selected = Some(next.len() - 1);
                }
                self.set_guides(next, step);
            }
        }
    }
}

/// Distance from `p` to the segment from `a` to `b`.
fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0., 1.);
    p.distance(a + ab * t)
}

/// A guide as Lightroom draws it: a line with a dark outline and its ends marked.
fn guide_line(painter: &egui::Painter, a: Pos2, b: Pos2, selected: bool) {
    let color = if selected {
        Color32::from_rgb(255, 214, 80)
    } else {
        Color32::WHITE
    };
    painter.line_segment([a, b], Stroke::new(3., Color32::from_black_alpha(140)));
    painter.line_segment([a, b], Stroke::new(1.2, color));
    for end in [a, b] {
        painter.circle_filled(end, 4.5, Color32::from_black_alpha(140));
        painter.circle_filled(end, 3., color);
    }
}

/// A square grid over `rect`, squares a twelfth of its long edge, as the Crop tool's.
fn grid_lines(painter: &egui::Painter, rect: Rect) {
    let guides = super::crop_tool::CropGuides {
        guide: super::crop_tool::Guide::Grid,
        ..Default::default()
    };
    for line in guides.lines(rect.size()) {
        let points: Vec<Pos2> = line.iter().map(|p| rect.min + p.to_vec2()).collect();
        painter.add(egui::Shape::line(
            points,
            Stroke::new(1., Color32::from_white_alpha(90)),
        ));
    }
}

/// Lightroom's loupe while a guide's end is placed: the photo around `at`, magnified,
/// with a cross on the point, beside the pointer and inside `area`.
fn magnifier(ui: &egui::Ui, texture: egui::TextureId, photo: Rect, at: Pos2, area: Rect) {
    const SIZE: f32 = 120.;
    const MAGNIFY: f32 = 4.;
    let painter = ui
        .ctx()
        .layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("guided-upright-loupe"),
        ))
        .with_clip_rect(area);
    // Above and to the right of the pointer, moved to stay in the photo area.
    let gap = Vec2::new(24., -24. - SIZE);
    let mut min = at + gap;
    if min.x + SIZE > area.right() {
        min.x = at.x - 24. - SIZE;
    }
    if min.y < area.top() {
        min.y = at.y + 24.;
    }
    let frame = Rect::from_min_size(min, Vec2::splat(SIZE));
    // The part of the texture around `at`, a magnified loupe's worth.
    let centre = (at - photo.min) / photo.size();
    let half = Vec2::splat(SIZE / MAGNIFY / 2.) / photo.size();
    let uv = Rect::from_min_max(
        Pos2::new(centre.x - half.x, centre.y - half.y),
        Pos2::new(centre.x + half.x, centre.y + half.y),
    );
    painter.rect_filled(frame.expand(2.), 3., Color32::from_black_alpha(200));
    painter.rect_filled(frame, 0., super::theme::palette(ui.ctx()).photo_backdrop());
    painter.image(texture, frame, uv, Color32::WHITE);
    let c = frame.center();
    for d in [Vec2::new(8., 0.), Vec2::new(0., 8.)] {
        painter.line_segment(
            [c - d, c + d],
            Stroke::new(1., Color32::from_black_alpha(160)),
        );
        painter.line_segment([c - d * 0.6, c + d * 0.6], Stroke::new(1., Color32::WHITE));
    }
    painter.rect_stroke(
        frame,
        0.,
        Stroke::new(1., Color32::from_white_alpha(200)),
        egui::StrokeKind::Outside,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(p: Pos2) -> [f32; 2] {
        [p.x / 100., p.y / 100.]
    }

    /// A drag adds a guide, or moves the end it started on; a short drag or a fifth
    /// guide adds nothing.
    #[test]
    fn released_drags_add_or_move_guides() {
        let one = UprightGuide {
            a: [0.1, 0.1],
            b: [0.1, 0.9],
        };
        let added = released(
            &[one],
            GuideDrag::New {
                from: Pos2::new(50., 10.),
                to: Pos2::new(52., 90.),
            },
            frame,
        )
        .expect("a guide");
        assert_eq!(added.len(), 2);
        assert_eq!(added[1].b, [0.52, 0.9]);
        let short = GuideDrag::New {
            from: Pos2::new(50., 10.),
            to: Pos2::new(53., 14.),
        };
        assert_eq!(released(&[one], short, frame), None);
        let moved = released(
            &[one],
            GuideDrag::End {
                guide: 0,
                end: End::B,
                to: Pos2::new(20., 80.),
            },
            frame,
        )
        .expect("moved");
        assert_eq!(
            moved,
            vec![UprightGuide {
                a: one.a,
                b: [0.2, 0.8]
            }]
        );
        let full = [one; crate::model::transform::MAX_GUIDES];
        let fifth = GuideDrag::New {
            from: Pos2::new(50., 10.),
            to: Pos2::new(52., 90.),
        };
        assert_eq!(released(&full, fifth, frame), None);
    }

    #[test]
    fn distance_to_a_segment_stops_at_its_ends() {
        let (a, b) = (Pos2::new(0., 0.), Pos2::new(10., 0.));
        assert_eq!(distance_to_segment(Pos2::new(5., 3.), a, b), 3.);
        assert_eq!(distance_to_segment(Pos2::new(14., 3.), a, b), 5.);
    }
}
