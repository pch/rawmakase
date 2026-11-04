//! The Masking drawer's side of Subject and Background: the two actions, what
//! they are waiting for, and what went wrong. Everything stays in the drawer; a
//! selection that fails or is cancelled leaves the photo as it was.
use super::{Failure, Feature, Pack, Prompt, Request, Target};
use crate::app::Editor;
use crate::app::develop::tools::retouch_tool::{control_label, hint, indented};
use crate::app::ui::design::{self, Tone, radius, space, text};
use eframe::egui;

impl Editor {
    /// The Select Subject and Select Background actions, with whatever the running or
    /// failed selection has to say. While the first use is being set up, the actions
    /// give way to a card that says what is needed, in order, and does it.
    pub(in crate::app) fn selection_actions(&mut self, ui: &mut egui::Ui) {
        if let Some(request) = self.selection.running() {
            self.selection_progress(ui, request);
            return;
        }
        if self.selection.prompting.is_some() {
            self.prompting_card(ui);
            return;
        }
        if self.selection.prompt.is_some() || self.selection.upgrading.is_some() {
            self.setup_card(ui);
            return;
        }
        let reason = self.selection_unavailable();
        control_label(ui, "Select", |ui| {
            let gap = 4.;
            let w = (ui.available_width() - 2. * gap) / 3.;
            for (feature, tip) in [
                (Feature::Subject, "Mask the photo's main subject"),
                (Feature::Sky, "Mask the sky"),
                (
                    Feature::Background,
                    "Mask everything but the photo's main subject",
                ),
            ] {
                if self.feature_tile(ui, feature, w, tip, reason) {
                    self.request_selection(Request {
                        feature,
                        target: Target::NewMask,
                    });
                }
            }
        });
        if let Some(why) = reason {
            hint(ui, why);
        }
        if let Some((request, failure)) = self.selection.failure.clone() {
            self.selection_failure(ui, request, &failure);
        }
        self.model_footer(ui);
    }
    /// One of the Select tiles: an icon over its name, as Lightroom's Add New Mask.
    /// Returns whether it was clicked; when `disabled` says why, it is dimmed and says so
    /// on hover.
    fn feature_tile(
        &self,
        ui: &mut egui::Ui,
        feature: Feature,
        width: f32,
        tip: &str,
        disabled: Option<&str>,
    ) -> bool {
        let palette = crate::app::ui::theme::palette(ui.ctx());
        let sense = if disabled.is_some() {
            egui::Sense::hover()
        } else {
            egui::Sense::click()
        };
        let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 52.), sense);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                disabled.is_none(),
                format!("Select {}", feature.name()),
            )
        });
        let hot = response.hovered() && disabled.is_none();
        ui.painter().rect_filled(
            rect,
            radius::CONTROL,
            palette.tone(if hot { Tone::OUTLINE } else { Tone::FIELD }),
        );
        let ink = palette.tone(match (disabled.is_some(), hot) {
            (true, _) => Tone::DISABLED,
            (false, true) => Tone::BRIGHT,
            (false, false) => Tone::SOFT,
        });
        paint_feature_icon(
            ui.painter(),
            rect.center_top() + egui::vec2(0., 20.),
            feature,
            ink,
        );
        ui.painter().text(
            rect.center_bottom() - egui::vec2(0., 9.),
            egui::Align2::CENTER_CENTER,
            feature.name(),
            egui::FontId::proportional(text::SMALL),
            ink,
        );
        let response = match disabled {
            Some(why) => response.on_hover_text(why),
            None => response
                .on_hover_text(tip)
                .on_hover_cursor(egui::CursorIcon::PointingHand),
        };
        response.clicked()
    }
    /// While aiming: what to do on the photo, and how to finish.
    fn prompting_card(&mut self, ui: &mut egui::Ui) {
        let Some(p) = &self.selection.prompting else {
            return;
        };
        let (feature, points, boxed, applied) = (
            p.request.feature,
            p.points.len(),
            p.bounds.is_some(),
            p.applied.is_some(),
        );
        let accent = egui::Color32::from_rgb(96, 150, 230);
        let failure = self.selection.failure.as_ref().map(|f| f.1.message());
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(34, 44, 62))
            .stroke(egui::Stroke::new(1.5, accent))
            .corner_radius(radius::CONTROL)
            .inner_margin(design::margin_same(space::M))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(format!(
                        "Select {}: click it on the photo",
                        feature.name()
                    ))
                    .strong()
                    .size(text::LABEL),
                );
                ui.add_space(space::XS);
                let small = |text: &str| {
                    egui::RichText::new(text)
                        .size(text::SMALL)
                        .color(egui::Color32::LIGHT_GRAY)
                };
                ui.add(
                    egui::Label::new(small(if applied {
                        "Click more of the subject to add to it, Alt-click something to leave \
                         it out, or drag a box to start over from the box."
                    } else {
                        "Click the subject, or drag a box around it. Each click can be refined \
                         with more: click to add, Alt-click to leave out."
                    }))
                    .wrap(),
                );
                let used = format!(
                    "{points} point{}{}",
                    if points == 1 { "" } else { "s" },
                    if boxed { " and a box" } else { "" }
                );
                ui.add(egui::Label::new(small(&used)));
                if let Some(why) = failure {
                    ui.colored_label(egui::Color32::from_rgb(230, 170, 90), why);
                }
                ui.add_space(space::XS);
                ui.horizontal(|ui| {
                    let done = egui::Button::new(egui::RichText::new("Done").strong())
                        .fill(egui::Color32::from_rgb(52, 98, 170));
                    if ui.add(done).on_hover_text("Escape").clicked() {
                        self.selection.end_prompting();
                    }
                    if ui
                        .button("Start over")
                        .on_hover_text("Forget the clicks; the next click selects afresh")
                        .clicked()
                    {
                        self.clear_prompt();
                    }
                });
            });
    }
    /// Clicks, drags and markers on the photo while aiming. Returns that the tool owns
    /// the pointer.
    pub(in crate::app) fn prompt_overlay(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        rect: egui::Rect,
        to_screen: &dyn Fn([f32; 2]) -> egui::Pos2,
        to_image: &dyn Fn(egui::Pos2) -> [f32; 2],
    ) -> bool {
        let busy = self.selection.running().is_some();
        if !busy {
            if response.drag_started()
                && let Some(origin) = ui.input(|i| i.pointer.press_origin())
            {
                self.selection.drag_from = Some(to_image(origin));
            }
            if response.drag_stopped()
                && let (Some(from), Some(pos)) = (
                    self.selection.drag_from.take(),
                    response.interact_pointer_pos(),
                )
            {
                self.prompt_box(from, to_image(pos));
            }
            if response.clicked()
                && let Some(pos) = response.interact_pointer_pos()
            {
                let alt = ui.input(|i| i.modifiers.alt);
                self.prompt_click(to_image(pos), !alt);
            }
        }
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let Some(p) = &self.selection.prompting else {
            return true;
        };
        if let Some(b) = p.bounds {
            let r = egui::Rect::from_two_pos(to_screen([b[0], b[1]]), to_screen([b[2], b[3]]));
            painter.rect_stroke(
                r,
                0.,
                egui::Stroke::new(1.5, egui::Color32::from_rgb(110, 200, 130)),
                egui::StrokeKind::Middle,
            );
        }
        if let (Some(from), Some(pos)) = (self.selection.drag_from, response.interact_pointer_pos())
            && response.dragged()
        {
            let r = egui::Rect::from_two_pos(to_screen(from), pos);
            painter.rect_stroke(
                r,
                0.,
                egui::Stroke::new(1.5, egui::Color32::WHITE),
                egui::StrokeKind::Middle,
            );
        }
        for point in &p.points {
            let at = to_screen([point.x, point.y]);
            let color = if point.positive {
                egui::Color32::from_rgb(110, 200, 130)
            } else {
                egui::Color32::from_rgb(230, 90, 80)
            };
            painter.circle(at, 6., color, egui::Stroke::new(1.5, egui::Color32::WHITE));
            if !point.positive {
                let d = egui::vec2(3., 3.);
                painter.line_segment(
                    [at - d, at + d],
                    egui::Stroke::new(1.5, egui::Color32::WHITE),
                );
                let e = egui::vec2(3., -3.);
                painter.line_segment(
                    [at - e, at + e],
                    egui::Stroke::new(1.5, egui::Color32::WHITE),
                );
            }
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(if busy {
                egui::CursorIcon::Progress
            } else {
                egui::CursorIcon::Crosshair
            });
        }
        true
    }
    /// What the first use needs, as a highlighted card in the place the actions were:
    /// the steps with the finished ones ticked, and the button for the next.
    fn setup_card(&mut self, ui: &mut egui::Ui) {
        let request = match self.selection.prompt {
            Some(Prompt::Model(r) | Prompt::Upgrade(r)) => Some(r),
            None => self.selection.upgrading.map(|(_, r)| r),
        };
        let Some(request) = request else { return };
        let upgraded = self
            .library
            .as_ref()
            .and_then(|l| l.session.catalog().supports_raster_masks().ok())
            .unwrap_or(false);
        let pack = request.feature.pack();
        let model = self.selection.models.of(pack).installed();
        let upgrading = self.selection.upgrading.is_some();
        let installing = self.selection.models.of(pack).busy();
        let accent = egui::Color32::from_rgb(96, 150, 230);
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(34, 44, 62))
            .stroke(egui::Stroke::new(1.5, accent))
            .corner_radius(radius::CONTROL)
            .inner_margin(design::margin_same(space::M))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(format!(
                        "Before “Select {}” can run",
                        request.feature.name()
                    ))
                    .strong()
                    .size(text::LABEL),
                );
                ui.add_space(space::XS);
                let step = |ui: &mut egui::Ui, done: bool, current: bool, text: &str| {
                    ui.horizontal(|ui| {
                        let (mark, color) = if done {
                            ("✓", egui::Color32::from_rgb(110, 200, 130))
                        } else if current {
                            ("▶", accent)
                        } else {
                            ("○", egui::Color32::GRAY)
                        };
                        ui.label(egui::RichText::new(mark).color(color).strong());
                        ui.label(egui::RichText::new(text).color(if done {
                            egui::Color32::GRAY
                        } else {
                            egui::Color32::WHITE
                        }));
                    });
                };
                step(ui, upgraded, !upgraded, "Upgrade this catalog");
                step(
                    ui,
                    model,
                    upgraded && !model,
                    &format!(
                        "Download the {} ({} MB)",
                        match pack {
                            Pack::Subject => "selection models",
                            Pack::Sky => "sky model",
                            Pack::Denoise => "AI Denoise model",
                        },
                        pack.megabytes()
                    ),
                );
                ui.add_space(space::S);
                if !upgraded {
                    self.upgrade_step(ui, request, upgrading);
                } else if let Some(endpoint) = self.selection.remote.endpoint() {
                    ui.colored_label(
                        egui::Color32::from_rgb(110, 200, 130),
                        format!(
                            "Selections run on the worker at {endpoint}; nothing has to be \
                                 downloaded here."
                        ),
                    );
                } else if !model {
                    self.model_step(ui, pack, installing);
                }
                if self.selection.remote.endpoint().is_none() && !super::worker::runtime_present() {
                    ui.add_space(space::XS);
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 170, 90),
                        "This copy of RAWmakase has no ONNX Runtime library, so selecting \
                         cannot run yet.",
                    );
                }
            });
    }
    fn upgrade_step(&mut self, ui: &mut egui::Ui, request: Request, upgrading: bool) {
        let small = |text: &str| {
            egui::RichText::new(text)
                .size(text::SMALL)
                .color(egui::Color32::LIGHT_GRAY)
        };
        ui.add(
            egui::Label::new(small(
                "Masks made from a selection are stored in the catalog, which needs a newer \
                 format. RAWmakase first saves a backup copy beside the catalog (as large as \
                 it), then upgrades it. Older versions of RAWmakase cannot open an upgraded \
                 catalog, and it must not be open on another computer or in another copy of \
                 RAWmakase.",
            ))
            .wrap(),
        );
        ui.add_space(space::S);
        if upgrading {
            ui.horizontal(|ui| {
                crate::app::ui::widgets::spinner(ui, 14.);
                ui.label("Backing up and upgrading the catalog…");
            });
            return;
        }
        ui.horizontal(|ui| {
            let go = egui::Button::new(egui::RichText::new("Upgrade catalog…").strong())
                .fill(egui::Color32::from_rgb(52, 98, 170));
            if ui.add(go).clicked() {
                self.upgrade_catalog(request);
            }
            if ui.button("Not now").clicked() {
                self.selection.prompt = None;
            }
        });
    }
    fn model_step(&mut self, ui: &mut egui::Ui, pack: Pack, installing: bool) {
        if let Some((done, total)) = self.selection.models.of(pack).progress() {
            ui.add(
                egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                    .desired_width(ui.available_width())
                    .text(format!("{} of {} MB", done / 1_000_000, total / 1_000_000)),
            );
            if ui.button("Cancel download").clicked() {
                self.selection.models.of_mut(pack).cancel();
            }
            return;
        }
        if installing {
            ui.horizontal(|ui| {
                crate::app::ui::widgets::spinner(ui, 14.);
                ui.label("Working…");
            });
            return;
        }
        if let Some(why) = self.selection.models.of(pack).failure() {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "The download stopped: {why}. Download again to carry on from \
                         where it stopped."
                    ))
                    .size(text::SMALL)
                    .color(egui::Color32::from_rgb(230, 170, 90)),
                )
                .wrap(),
            );
            ui.add_space(space::XS);
        }
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "The model runs on this computer; no photo leaves it. It is a one-time \
                     download and needs about that much free disk space.",
                )
                .size(text::SMALL)
                .color(egui::Color32::LIGHT_GRAY),
            )
            .wrap(),
        );
        ui.add_space(space::S);
        ui.horizontal(|ui| {
            let go = egui::Button::new(egui::RichText::new("Download").strong())
                .fill(egui::Color32::from_rgb(52, 98, 170));
            if ui.add(go).clicked() {
                self.install_model(pack);
            }
            if ui.button("Not now").clicked() {
                self.selection.prompt = None;
            }
        });
    }
    /// Remove, once a model is installed.
    fn model_footer(&mut self, ui: &mut egui::Ui) {
        if self.selection.running().is_some() {
            return;
        }
        for (pack, label) in [
            (Pack::Subject, "Remove selection model"),
            (Pack::Sky, "Remove sky model"),
        ] {
            if !self.selection.models.of(pack).installed() {
                continue;
            }
            indented(ui, |ui| {
                let remove = ui
                    .small_button(label)
                    .on_hover_text(
                        "Delete the downloaded model. Saved masks keep working; selecting \
                         again asks to download it.",
                    )
                    .clicked();
                if remove {
                    let unload = self.selection.worker.unloader();
                    self.selection.models.of_mut(pack).remove(
                        unload,
                        self.tx.clone(),
                        self.context.clone(),
                    );
                }
            });
        }
    }
    fn selection_progress(&mut self, ui: &mut egui::Ui, request: Request) {
        control_label(ui, "Select", |ui| {
            crate::app::ui::widgets::spinner(ui, 14.);
            // No percentage: the runtime reports none for a run.
            let where_at = self
                .selection
                .remote
                .endpoint()
                .map(|endpoint| format!(" (on {endpoint})"))
                .unwrap_or_default();
            ui.label(format!(
                "Finding the {}…{where_at}",
                request.feature.name().to_lowercase()
            ))
            .on_hover_text(
                "The first selection on a photo takes a few seconds; the next ones are quick",
            );
            if ui
                .button("Cancel")
                .on_hover_text("Stop selecting; the photo is left as it was")
                .clicked()
            {
                self.cancel_selection();
            }
        });
    }
    fn selection_failure(&mut self, ui: &mut egui::Ui, request: Request, failure: &Failure) {
        if *failure == Failure::ModelMissing {
            self.selection.failure = None;
            self.selection.prompt = Some(Prompt::Model(request));
            return;
        }
        hint(ui, &failure.message());
        if *failure == Failure::NoSubject {
            hint(
                ui,
                "Automatic selection knows people and animals. For anything else, click it yourself.",
            );
        }
        indented(ui, |ui| {
            if *failure == Failure::NoSubject
                && ui
                    .add(
                        egui::Button::new(egui::RichText::new("Click the subject…").strong())
                            .fill(egui::Color32::from_rgb(52, 98, 170)),
                    )
                    .clicked()
            {
                self.begin_aiming(request);
            }
            if ui.button("Try again").clicked() {
                self.selection.failure = None;
                // A failed click is retried with its clicks, not as an automatic selection.
                if self.selection.prompting.is_some() {
                    self.run_prompt();
                } else {
                    self.request_selection(request);
                }
            }
            if ui.button("Dismiss").clicked() {
                self.selection.failure = None;
            }
        });
    }
    /// Subject and Background entries for a mask's Add, Subtract and Intersect menus.
    pub(in crate::app) fn selection_menu(
        &mut self,
        ui: &mut egui::Ui,
        mask: usize,
        op: crate::model::masks::MaskOp,
    ) {
        ui.separator();
        let reason = self.selection_unavailable();
        for (feature, label) in [
            (Feature::Subject, "Subject"),
            (Feature::Sky, "Sky"),
            (Feature::Background, "Background"),
        ] {
            let response = ui.add_enabled(
                reason.is_none() && self.selection.running().is_none(),
                egui::Button::new(label),
            );
            let response = match reason {
                Some(why) => response.on_disabled_hover_text(why),
                None => response,
            };
            if response.clicked() {
                self.request_selection(Request {
                    feature,
                    target: Target::Component { mask, op },
                });
                ui.close();
            }
        }
    }
    /// The selected generated component's source, Regenerate and, for subjects, aiming
    /// with clicks.
    pub(in crate::app) fn regenerate_ui(
        &mut self,
        ui: &mut egui::Ui,
        mask: usize,
        component: usize,
        feature: Feature,
        source: &str,
    ) {
        hint(ui, source);
        let reason = self.selection_unavailable();
        let busy = self.selection.running().is_some();
        let free = reason.is_none() && !busy && self.selection.prompting.is_none();
        indented(ui, |ui| {
            let regenerate = ui.add_enabled(free, egui::Button::new("Regenerate"));
            let regenerate = match reason {
                Some(why) => regenerate.on_disabled_hover_text(why),
                None => regenerate.on_hover_text(
                    "Find it again and replace only this part of the mask; its other parts and \
                     the adjustments stay",
                ),
            };
            if regenerate.clicked() {
                self.request_selection(Request {
                    feature,
                    target: Target::Regenerate { mask, component },
                });
            }
            if feature != Feature::Sky {
                let refine = ui
                    .add_enabled(free, egui::Button::new("Refine with clicks…"))
                    .on_hover_text(
                        "Click on the photo to add to this selection, Alt-click to leave \
                         something out",
                    );
                if refine.clicked() {
                    self.begin_refining(mask, component);
                }
            }
        });
        if let Some(request) = self.selection.running() {
            self.selection_progress(ui, request);
        }
        if let Some((request, failure)) = self.selection.failure.clone() {
            self.selection_failure(ui, request, &failure);
        }
        if self.selection.prompt.is_some() || self.selection.upgrading.is_some() {
            self.setup_card(ui);
        }
    }
}

/// An icon for each selection, in a dashed frame as Lightroom draws them: a head and
/// shoulders, a sky over a hill, and the frame with the head and shoulders cut out.
pub(in crate::app) fn paint_feature_icon(
    painter: &egui::Painter,
    c: egui::Pos2,
    feature: Feature,
    ink: egui::Color32,
) {
    let stroke = egui::Stroke::new(1.2, ink);
    let (w, h) = (11., 9.5);
    let corners = [
        c + egui::vec2(-w, -h),
        c + egui::vec2(w, -h),
        c + egui::vec2(w, h),
        c + egui::vec2(-w, h),
        c + egui::vec2(-w, -h),
    ];
    painter.add(egui::Shape::dashed_line(&corners, stroke, 2.2, 1.8));
    // A head and shoulders, centred a little low.
    let person = |color: egui::Color32| {
        painter.circle_filled(c + egui::vec2(0., -2.6), 2.9, color);
        painter.add(egui::Shape::convex_polygon(
            (0..=12)
                .map(|i| {
                    let t = std::f32::consts::PI * i as f32 / 12.;
                    c + egui::vec2(-t.cos() * 5.8, 8.4 - t.sin() * 5.4)
                })
                .collect(),
            color,
            egui::Stroke::NONE,
        ));
    };
    match feature {
        Feature::Subject => person(ink),
        Feature::Sky => {
            painter.circle_filled(c + egui::vec2(5., -3.8), 1.9, ink);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    c + egui::vec2(-8., 7.),
                    c + egui::vec2(-3., -1.),
                    c + egui::vec2(0.5, 3.),
                    c + egui::vec2(3., 0.),
                    c + egui::vec2(8., 7.),
                ],
                ink,
                egui::Stroke::NONE,
            ));
        }
        Feature::Background => {
            painter.rect_filled(
                egui::Rect::from_center_size(c, egui::vec2(2. * w - 3., 2. * h - 3.)),
                1.,
                ink.gamma_multiply(0.55),
            );
            // The subject cut out of it: drawn in the tile's own colour.
            person(egui::Color32::from_gray(38));
        }
    }
}
