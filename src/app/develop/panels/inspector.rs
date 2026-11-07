use crate::app::develop::clipping::{self, ClipSide};
use crate::app::develop::panels::tone_drag::tone_drag_ui;
use crate::app::develop::state::Tool;
use crate::app::develop::step_name::StepName;
use crate::app::develop::tools::crop_tool::{Guide, GuideShow, Ruler};
use crate::app::shell::dialogs::FileDialog;
use crate::app::ui::design::{Tone, button, inset, radius, size, space, text};
use crate::app::ui::icons::{self, Icon};
use crate::app::ui::theme;
use crate::app::ui::widgets::{
    LABEL_GAP, SliderEvent, control_left, keyed_setting_slider, label_column, setting_slider,
    slider_with,
};
use crate::app::{Editor, develop::panels::basic_keys};
use crate::model::panels::{Panel, PanelState};
use crate::model::params::ParameterId;
use crate::model::recipe::Recipe;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use panels::transform::GuidedAction;

mod ai_denoise;
mod lens_profile;
mod lensfun;
mod panels;
mod profile_offer;
/// What the community profile offer says when a profile can be downloaded, for tests.
#[cfg(test)]
pub(in crate::app) fn profile_offer_available() -> &'static str {
    profile_offer::AVAILABLE
}
/// Lensfun's terms, for Preferences.
pub(in crate::app) use lensfun::terms as lensfun_terms;
mod profile_menu;

/// A histogram corner's clipping triangle: its corner point, which way it
/// points (1 right, -1 left) and the area that takes its clicks.
#[derive(Clone, Copy)]
struct ClipTriangle {
    corner: Pos2,
    dir: f32,
    hit: Rect,
}
impl ClipTriangle {
    fn new(histogram: Rect, side: ClipSide) -> Self {
        let (corner, dir) = match side {
            ClipSide::Shadows => (histogram.left_top() + Vec2::new(6., 6.), 1.),
            ClipSide::Highlights => (histogram.right_top() + Vec2::new(-6., 6.), -1.),
        };
        Self {
            corner,
            dir,
            hit: Rect::from_center_size(corner + Vec2::new(4. * dir, 3.), Vec2::splat(16.)),
        }
    }
}

impl Editor {
    /// Lightroom-style histogram: filled channels whose overlaps mix to
    /// cyan, magenta, yellow and gray, with clipping indicators in the corners.
    /// Dragging in it moves Blacks, Shadows, Exposure, Highlights or Whites.
    pub(in crate::app) fn histogram_ui(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 96.), Sense::hover());
        let painter = ui.painter().clone();
        painter.rect_filled(rect, radius::SMALL, palette.tone(Tone::WELL));
        for i in 1..5 {
            let x = rect.left() + i as f32 / 5. * rect.width();
            painter.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                Stroke::new(1., palette.tone(Tone::CARD)),
            );
        }
        let histogram = self.preview.histogram;
        let h = histogram.bins;
        // Light smoothing, and scaling that ignores the clipped end bins.
        let smooth = |c: usize, i: usize| {
            let at = |j: isize| h[c][j.clamp(0, 255) as usize] as f32;
            let i = i as isize;
            (at(i - 1) + 2. * at(i) + at(i + 1)) / 4.
        };
        let max = (2..254)
            .flat_map(|i| (0..3).map(move |c| (c, i)))
            .map(|(c, i)| smooth(c, i))
            .fold(1., f32::max);
        let plot = rect.shrink2(Vec2::new(0., 4.));
        let bar = plot.width() / 256.;
        let colors = [
            Color32::from_rgb(196, 58, 52),
            Color32::from_rgb(62, 170, 70),
            Color32::from_rgb(56, 104, 220),
        ];
        let pair = |a: usize, b: usize| match (a.min(b), a.max(b)) {
            (0, 1) => Color32::from_rgb(190, 176, 60),
            (1, 2) => Color32::from_rgb(58, 168, 186),
            _ => Color32::from_rgb(170, 70, 170),
        };
        for i in 0..256 {
            let mut v: Vec<(f32, usize)> = (0..3)
                .map(|c| ((smooth(c, i) / max).sqrt().min(1.) * plot.height(), c))
                .collect();
            v.sort_by(|a, b| a.0.total_cmp(&b.0));
            let x = plot.left() + i as f32 * bar;
            let segment = |from: f32, to: f32, color: Color32| {
                if to > from {
                    painter.rect_filled(
                        Rect::from_min_max(
                            Pos2::new(x, plot.bottom() - to),
                            Pos2::new(x + bar + 0.5, plot.bottom() - from),
                        ),
                        0.,
                        color,
                    );
                }
            };
            segment(0., v[0].0, palette.tone(Tone::SECONDARY));
            segment(v[0].0, v[1].0, pair(v[1].1, v[2].1));
            segment(v[1].0, v[2].0, colors[v[2].1]);
        }
        // After the bars, so the region shows over them; before the triangles,
        // so their clicks stay theirs.
        let triangles = ClipSide::BOTH.map(|side| ClipTriangle::new(rect, side));
        let region = tone_drag_ui(
            ui,
            &mut self.step_name,
            rect,
            &triangles.map(|t| t.hit),
            &mut self.view.tone_drag,
            self.document.edit.recipe_mut(),
        );
        if let Some(region) = region {
            let [from, to] = region.span();
            painter.rect_filled(
                Rect::from_x_y_ranges(
                    rect.left() + from * rect.width()..=rect.left() + to * rect.width(),
                    rect.y_range(),
                ),
                0.,
                Color32::from_white_alpha(14),
            );
        }
        // Clipping triangles, as Lightroom's: each lit in the colours of the
        // channels clipping at its end; a click toggles its warning, hovering
        // shows it while the pointer stays, and J toggles both.
        for (side, ClipTriangle { corner, dir, hit }) in ClipSide::BOTH.into_iter().zip(triangles) {
            let left = side == ClipSide::Shadows;
            let response = ui
                .interact(hit, ui.id().with(("clip", left)), Sense::click())
                .on_hover_text(if left {
                    "Show shadow clipping · J shows both"
                } else {
                    "Show highlight clipping · J shows both"
                });
            if response.clicked() {
                self.view.clipping.toggle(side);
            }
            if response.hovered() {
                self.view.clipping.set_hover(Some(side));
            }
            let on = self.view.clipping.is_on(side);
            let color = clipping::indicator_color(clipping::clipped_channels(&histogram, side))
                .unwrap_or_else(|| {
                    palette.tone(if on || response.hovered() {
                        Tone::SECONDARY
                    } else {
                        Tone::OUTLINE_STRONG
                    })
                });
            painter.add(egui::Shape::convex_polygon(
                vec![
                    corner,
                    corner + Vec2::new(9. * dir, 0.),
                    corner + Vec2::new(0., 7.),
                ],
                color,
                if on {
                    Stroke::new(1., Color32::WHITE)
                } else {
                    Stroke::NONE
                },
            ));
        }
        // The region and its value take the EXIF line's place, as in Lightroom.
        let region_text = region.map(|region| {
            format!(
                "{}   {}",
                region.label(),
                region.display(region.value(self.document.edit.recipe()))
            )
        });
        // So does the RGB readout while the pointer is over the photo.
        let exif = region_text
            .or_else(|| self.view.readout.text())
            .unwrap_or_else(|| {
                self.document
                    .metadata
                    .as_ref()
                    .map_or_else(String::new, |m| {
                        let info = crate::metadata::PhotoInfo::from_metadata(m);
                        [
                            info.iso_text(),
                            info.focal_text(),
                            info.aperture_text(),
                            info.shutter_text(),
                        ]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join("     ")
                    })
            });
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new(exif)
                    .size(text::SMALL)
                    .color(palette.tone(Tone::LABEL)),
            )
            .on_hover_text("Output histogram of the whole photo");
        });
    }
    /// The open tool's drawer, in place of Develop's adjustments: Crop's
    /// settings, or Remove's, Red Eye's or Masking's. Its tools are on the
    /// rail.
    pub(in crate::app) fn tool_drawer(&mut self, ui: &mut egui::Ui) {
        let Some(name) = DRAWER_TOOLS
            .iter()
            .find(|(tool, ..)| *tool == self.view.tool)
            .map(|(.., name)| *name)
        else {
            return;
        };
        crate::app::ui::widgets::panel_caption(ui, name);
        let drawer = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
            drawer_frame().show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(space::XS, space::S);
                add(ui)
            });
        };
        match self.view.tool {
            Tool::Remove => {
                return drawer(ui, &mut |ui| {
                    experimental(ui, UNMEASURED);
                    self.retouch_panel(ui)
                });
            }
            Tool::RedEye => {
                return drawer(ui, &mut |ui| {
                    experimental(ui, RED_EYE_NOTE);
                    self.red_eye_panel(ui)
                });
            }
            Tool::Mask => {
                return drawer(ui, &mut |ui| {
                    experimental(ui, UNMEASURED);
                    self.mask_panel(ui)
                });
            }
            Tool::Crop => {}
            _ => return,
        }
        let palette = theme::palette(ui.ctx());
        // The chosen aspect is the lock; `view.aspect` is read from the photo only
        // once it is decoded.
        let locked = self.view.chosen_aspect != 0.;
        // Unlocked, the menu still names the crop's own ratio, as Lightroom shows
        // Custom beside its open padlock.
        let shown = if locked {
            self.view.aspect
        } else {
            self.crop_frame().map_or(0., |photo| {
                crate::app::develop::tools::crop_tool::crop_aspect_shown(
                    self.document.edit.recipe().crop,
                    photo,
                )
            })
        };
        let mut action = None;
        let mut chosen = None;
        let mut guides = self.view.crop_guides;
        let analysing = self.document.straighten.is_running();
        let r = self.document.edit.recipe_mut();
        let step = &mut self.step_name;
        drawer_frame().show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(space::XS, space::S);
            // Every control one height, centred on its label, at the Develop
            // panels' text size.
            ui.spacing_mut().button_padding = Vec2::new(space::M, space::XXS);
            ui.spacing_mut().interact_size.y = CROP_CONTROL;
            ui.style_mut().text_styles.insert(
                egui::TextStyle::Button,
                egui::FontId::proportional(text::LABEL),
            );
            control_row(ui, "Aspect", |ui| {
                let w = ui.available_width() - 2. * (CROP_CONTROL + 4.);
                if aspect_lock(ui, locked).clicked() {
                    action = Some(CropAction::ToggleLock);
                }
                egui::ComboBox::from_id_salt("crop-aspect")
                    .width(w)
                    .selected_text(aspect_name(shown))
                    .show_ui(ui, |ui| {
                        for (aspect, name, _) in ASPECTS {
                            if ui
                                .selectable_label(self.view.aspect == aspect, name)
                                .clicked()
                            {
                                chosen = Some(aspect);
                            }
                        }
                    });
                if crop_icon_button(ui, Icon::SwapOrientation, CROP_CONTROL)
                    .on_hover_text("Swap portrait and landscape · X")
                    .clicked()
                {
                    action = Some(CropAction::Swap);
                }
            });
            crate::model::edit::change_setting(r, ParameterId::Straighten, None, |r| {
                setting_slider(ui, step, ParameterId::Straighten, &mut r.straighten, 0.)
            });
            control_row(ui, "Straighten", |ui| {
                let w = (ui.available_width() - 4.) / 2.;
                let armed = self.view.ruler == Ruler::Armed;
                if ui
                    .add_sized(
                        [w, CROP_CONTROL],
                        egui::Button::new("Ruler").selected(armed),
                    )
                    .on_hover_text(
                        "Drag along a horizon or vertical to level it · or Cmd-drag on the photo",
                    )
                    .clicked()
                {
                    self.view.ruler = if armed { Ruler::Off } else { Ruler::Armed };
                }
                if ui
                    .add_enabled_ui(!analysing, |ui| {
                        ui.add_sized([w, CROP_CONTROL], egui::Button::new("Auto"))
                    })
                    .inner
                    .on_hover_text("Level the photo as Upright's Level would, by its angle alone")
                    .clicked()
                {
                    action = Some(CropAction::AutoStraighten);
                }
            });
            control_row(ui, "Orientation", |ui| {
                use crate::develop::{Mirror, QuarterTurn, mirror, turn};
                let w = (ui.available_width() - 3. * 4.) / 4.;
                for (icon, tip, apply) in [
                    (
                        Icon::RotateLeft,
                        "Rotate left",
                        (|r| turn(r, QuarterTurn::Left)) as fn(&mut Recipe),
                    ),
                    (Icon::RotateRight, "Rotate right", |r| {
                        turn(r, QuarterTurn::Right)
                    }),
                    (Icon::FlipHorizontal, "Flip horizontal", |r| {
                        mirror(r, Mirror::Horizontal)
                    }),
                    (Icon::FlipVertical, "Flip vertical", |r| {
                        mirror(r, Mirror::Vertical)
                    }),
                ] {
                    if crop_icon_button(ui, icon, w).on_hover_text(tip).clicked() {
                        apply(r);
                    }
                }
            });
            control_row(ui, "Overlay", |ui| {
                let show = 72.;
                egui::ComboBox::from_id_salt("crop-guide")
                    .width(ui.available_width() - show - 4.)
                    .selected_text(guides.guide.name())
                    .show_ui(ui, |ui| {
                        for guide in Guide::ALL {
                            if ui
                                .selectable_label(guides.guide == guide, guide.name())
                                .clicked()
                            {
                                guides.guide = guide;
                                guides.orientation = 0;
                            }
                        }
                    })
                    .response
                    .on_hover_text("O cycles the overlays, Shift+O turns them");
                egui::ComboBox::from_id_salt("crop-guide-show")
                    .width(show)
                    .selected_text(guides.show.name())
                    .show_ui(ui, |ui| {
                        for when in GuideShow::ALL {
                            ui.selectable_value(&mut guides.show, when, when.name());
                        }
                    })
                    .response
                    .on_hover_text(
                        "When the overlay shows: always, with the pointer over the photo, or never",
                    );
            });
            ui.add_space(space::S);
            ui.horizontal(|ui| {
                ui.add_space(control_left(ui));
                let w = (ui.available_width() - 4.) / 2.;
                if ui
                    .add_sized([w, 24.], egui::Button::new("Reset"))
                    .on_hover_text("Remove the crop, straightening, rotation and flips")
                    .clicked()
                {
                    r.crop = [0., 0., 1., 1.];
                    // An angle Reset sets is not Auto's, even when Auto's was 0.
                    r.straighten = 0.;
                    r.straighten_auto = None;
                    r.rotation = 0;
                    r.flip_x = false;
                    r.flip_y = false;
                }
                if ui
                    .add_sized(
                        [w, 24.],
                        egui::Button::new(
                            egui::RichText::new("Done").color(palette.on_accent_text(Tone::BRIGHT)),
                        )
                        .fill(palette.accent()),
                    )
                    .on_hover_text("Finish cropping · Enter or R")
                    .clicked()
                {
                    self.view.tool = Tool::None;
                }
            });
            ui.horizontal(|ui| {
                ui.add_space(control_left(ui));
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("Drag the frame or its corners on the photo.")
                            .size(text::MICRO)
                            .color(palette.tone(Tone::FAINT)),
                    )
                    .wrap(),
                );
            });
        });
        self.set_crop_guides(guides);
        if let Some(aspect) = chosen {
            self.choose_aspect(aspect);
        }
        match action {
            Some(CropAction::Swap) => self.swap_crop_orientation(),
            Some(CropAction::ToggleLock) => self.toggle_aspect_lock(),
            Some(CropAction::AutoStraighten) => self.start_auto_straighten(),
            None => {}
        }
    }
    pub(in crate::app) fn controls(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        ui.spacing_mut().item_spacing = Vec2::new(space::XS, space::XS);
        ui.spacing_mut().button_padding = button::COMPACT;
        ui.spacing_mut().interact_size.y = size::ROW;
        ui.add_space(space::S);
        // Lensfun's database as the Lens Corrections panel shows it this frame.
        let lensfun_view = lensfun::View {
            db: self.lensfun.database(),
            running: self.lensfun.running(),
            progress: self.lensfun.progress(),
            failure: self
                .lensfun
                .failure(crate::app::develop::lensfun::Job::Install)
                .map(str::to_string),
            suggest: self.lensfun.suggest,
        };
        let ai_view = ai_denoise::View {
            installed: self.selection.models.denoise.installed(),
            progress: self.selection.models.denoise.progress(),
            install_failure: self.selection.models.denoise.failure().map(str::to_string),
            megabytes: crate::model_install::Pack::Denoise.megabytes(),
            status: self.document.ai.status().cloned(),
            unavailable: self
                .document
                .opened_from_preview()
                .then_some(crate::app::develop::ai_denoise::CONNECT_FOR_AI_DENOISE),
        };
        // Cached per camera: this scans Adobe's profile folders.
        let adobe_key = self
            .document
            .metadata
            .as_ref()
            .map(|m| egui::Id::new(("adobe-profiles", &m.make, &m.model)));
        let adobe: Vec<std::path::PathBuf> = match (adobe_key, &self.document.metadata) {
            // A JPEG, TIFF or PNG has its Embedded profile only.
            (Some(key), Some(m)) if !m.is_rendered() => ui.ctx().data_mut(|d| {
                d.get_temp_mut_or_insert_with(key, || crate::camera_profiles::adobe_installed(m))
                    .clone()
            }),
            _ => Vec::new(),
        };
        // Community profiles from the catalogue: matched here, measured once per camera.
        let community_profiles = self.has_catalogue_profiles();
        let community_offer = self.profile_offer();
        let metadata = self.document.metadata.clone();
        let profiles = self.document.profiles.clone();
        let profile_errors = self.document.profile_errors.clone();
        let profile_groups = self.document.profile_groups.clone();
        let histogram = self.preview.histogram.bins;
        // Auto needs the decoded photo, and runs one estimate at a time.
        let auto_ready = self.document.full().is_some() && !self.document.auto.is_running();
        let auto_in_effect = self.auto_in_effect();
        // Upright analyses the decoded photo once and keeps a correction for every mode.
        let upright_ready = self.document.full().is_some() && !self.document.upright.is_running();
        let can_fit_camera_look = self.can_fit_camera_look();
        let fit_camera_look_unavailable = self.camera_look_unavailable();
        let hdr = self.hdr_presentation();
        // A conversion to black & white waiting for the photo to decode.
        let pending_treatment = self
            .document
            .pending_treatment
            .as_ref()
            .map(|p| p.treatment);
        let grading_document = self.document.edit.history().id();
        let saved_curves = self.saved_curves();
        // The Targeted Adjustment Tool: the sliders a drag is moving, and a target
        // button's click.
        let targeted = self.targeted_weights();
        let basic_view = panels::basic::View {
            metadata: metadata.as_ref(),
            profiles: &profiles,
            profile_errors: &profile_errors,
            profile_groups: &profile_groups,
            adobe: &adobe,
            community_profiles,
            community_offer: community_offer.as_ref(),
            pending_treatment,
            auto_ready,
            auto_in_effect,
            can_fit_camera_look,
            fit_camera_look_unavailable,
            hdr: &hdr,
        };
        let mut requests = panels::Requests::default();
        let view = &mut self.view;
        let step = &mut self.step_name;
        let (r, photo) = self.document.recipe_and_colors();
        let metadata = metadata.as_ref();
        requests.add(panels::basic::show(
            ui,
            step,
            r,
            view,
            &basic_view,
            &palette,
        ));
        requests.add(panels::tone_curve::show(
            ui,
            step,
            r,
            view,
            &histogram,
            targeted,
            &saved_curves,
        ));
        requests.add(panels::mixer::show(
            ui, step, r, view, &photo, targeted, &palette,
        ));
        panels::color_grading::show(ui, step, r, view, grading_document);
        requests.add(panels::detail::show(ui, step, r, &ai_view));
        requests.add(panels::lens_corrections::show(
            ui,
            step,
            r,
            view,
            metadata,
            &lensfun_view,
            &palette,
        ));
        requests.add(panels::transform::show(
            ui,
            step,
            r,
            view,
            metadata,
            upright_ready,
            &palette,
        ));
        panels::effects::show(ui, step, r, metadata);
        panels::calibration::show(ui, step, r, metadata);
        self.carry_out(requests, ui, &adobe, adobe_key);
    }
    /// Carries out what the Develop panels asked for this frame, still during the
    /// edit frame they were drawn in.
    fn carry_out(
        &mut self,
        requests: panels::Requests,
        ui: &egui::Ui,
        adobe: &[std::path::PathBuf],
        adobe_key: Option<egui::Id>,
    ) {
        let panels::Requests {
            auto: auto_request,
            profile_changed_from,
            treatment: treatment_request,
            fit_camera_look,
            auto_mix: auto_mix_request,
            curve: curve_choice,
            targeted: targeted_request,
            upright: upright_request,
            guided: guided_action,
            import_profiles,
            show_community_profiles,
            offer: offer_action,
            import_lens,
            ai_denoise: ai_action,
            lensfun: lensfun_action,
            import_folder,
            import_luts,
            import_adobe,
        } = requests;
        if let Some(kind) = auto_request {
            self.start_auto(kind);
        }
        if let Some(old) = profile_changed_from {
            self.follow_profile_treatment(old.as_deref());
        }
        if let Some(treatment) = treatment_request {
            self.set_treatment(treatment);
        }
        if fit_camera_look {
            self.start_camera_look();
        }
        if auto_mix_request {
            self.auto_black_white_mix();
        }
        if let Some(choice) = curve_choice {
            self.choose_point_curve(choice);
        }
        if let Some(target) = targeted_request {
            self.toggle_targeted(target);
        }
        if upright_request {
            self.start_upright();
        }
        match guided_action {
            Some(GuidedAction::Choose) => {
                if self.view.is(Tool::Guided) {
                    self.choose_guided();
                } else {
                    self.toggle_guided_tool();
                }
            }
            Some(GuidedAction::Toggle) => self.toggle_guided_tool(),
            Some(GuidedAction::Clear) => {
                self.view.guided.selected = None;
                self.set_guides(Vec::new(), "Clear Guides");
            }
            None => {}
        }
        if import_profiles {
            self.dialog(FileDialog::CameraProfile, &ui.ctx().clone());
        }
        if show_community_profiles && let Some(m) = self.document.metadata.clone() {
            self.profile_download.request(&m);
        }
        if let Some(action) = offer_action {
            self.profile_offer_action(action, &ui.ctx().clone());
        }
        if import_lens {
            self.dialog(FileDialog::LensProfile, &ui.ctx().clone());
        }
        match ai_action {
            Some(ai_denoise::Action::Install) => {
                self.install_model(crate::model_install::Pack::Denoise);
            }
            Some(ai_denoise::Action::Remove) => self.remove_ai_denoise_model(),
            Some(ai_denoise::Action::Cancel) => {
                self.selection
                    .models
                    .of_mut(crate::model_install::Pack::Denoise)
                    .cancel();
            }
            None => {}
        }
        match lensfun_action {
            Some(lensfun::Action::Download) => {
                self.lensfun.start(
                    crate::app::develop::lensfun::Job::Install,
                    self.tx.clone(),
                    ui.ctx().clone(),
                );
            }
            Some(lensfun::Action::Cancel) => self.lensfun.cancel(),
            None => {}
        }
        if let Some(kind) = import_folder {
            self.dialog(FileDialog::ImportFolder(kind), &ui.ctx().clone());
        }
        if import_luts {
            self.open_lut_import();
        }
        if import_adobe && let Some(m) = self.document.metadata.clone() {
            let result = crate::camera_profiles::import_readable(adobe);
            if let Some(key) = adobe_key {
                // Imported or unreadable, none of them is worth offering again.
                ui.ctx().data_mut(|d| {
                    if result.is_ok() {
                        d.insert_temp(key, Vec::<std::path::PathBuf>::new());
                    } else {
                        d.remove::<Vec<std::path::PathBuf>>(key);
                    }
                });
            }
            match result {
                Ok(done) => {
                    if !done.imported.is_empty() {
                        self.list_imported_profiles(&m);
                        ui.ctx().request_repaint();
                    }
                    let mut reasons: Vec<&str> =
                        done.skipped.iter().map(|(_, e)| e.as_str()).collect();
                    reasons.sort_unstable();
                    reasons.dedup();
                    let reasons = reasons.join("; ");
                    match (done.imported.len(), done.skipped.len()) {
                        (0, k) => self.messages.warn(format!(
                            "No new Adobe profiles for {} {}; skipped {k} ({reasons}).",
                            m.make, m.model
                        )),
                        (n, 0) => self.messages.done(format!(
                            "Imported {n} Adobe profiles for {} {}. Choose one from the Profile menu.",
                            m.make, m.model
                        )),
                        (n, k) => self.messages.warn(format!(
                            "Imported {n} Adobe profiles for {} {}, skipped {k} ({reasons}). Choose one from the Profile menu.",
                            m.make, m.model
                        )),
                    }
                }
                Err(e) => self.messages.fail(format!("Profiles not imported: {e:#}")),
            }
        }
    }
    /// Lists the open photo's profiles again after Adobe profiles were imported
    /// for its camera. As the loader's, the raw defaults are resolved again after
    /// this frame, so changing an unedited photo's recipe is not taken for an
    /// edit; the Library's previews follow them then, once it is known whether
    /// the open photo's recipe changed.
    pub(in crate::app) fn list_imported_profiles(&mut self, m: &crate::camera_data::Metadata) {
        self.profile_library = crate::camera_profiles::library_fingerprint();
        let crate::camera_profiles::Listed {
            profiles,
            errors,
            groups,
        } = crate::camera_profiles::listed(m);
        let profiles = crate::app::develop::camera_look::keep_fitted_default(
            &self.document.profiles,
            m,
            profiles,
        );
        let _ = self.tx.send(crate::app::worker::Event::Profiles {
            id: self.document.load_id(),
            profiles,
            errors,
            groups,
            imported: true,
        });
    }
}

/// A Crop panel button that acts once the panel is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CropAction {
    Swap,
    ToggleLock,
    AutoStraighten,
}

/// The aspect menu's name for `aspect`: a preset either way round (X swaps them),
/// Original or Free, or Custom.
fn aspect_name(aspect: f32) -> &'static str {
    let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
    ASPECTS
        .iter()
        .find(|(a, ..)| near(*a, aspect) || (*a > 0. && near(1. / *a, aspect)))
        .map_or("Custom", |(_, name, _)| *name)
}

/// The crop's aspect presets, long side over short, with their menu and stable
/// session names.
pub(in crate::app) const ASPECTS: [(f32, &str, &str); 9] = [
    (-1., "Original", "original"),
    (0., "Free", "free"),
    (1., "1 x 1", "1x1"),
    (1.25, "4 x 5", "4x5"),
    (1.4, "5 x 7", "5x7"),
    (1.5, "2 x 3", "2x3"),
    (4. / 3., "3 x 4", "3x4"),
    (16. / 9., "16 x 9", "16x9"),
    (65. / 24., "65 x 24 (XPan)", "65x24"),
];
/// The session's name for a preset `aspect` from the menu.
pub(in crate::app) fn aspect_key(aspect: f32) -> &'static str {
    ASPECTS
        .iter()
        .find(|(a, ..)| *a == aspect)
        .map_or("original", |(.., key)| *key)
}
/// The preset the session names `key`; Original for a name this version does not know.
pub(in crate::app) fn aspect_from_key(key: &str) -> f32 {
    ASPECTS
        .iter()
        .find(|(.., k)| *k == key)
        .map_or(-1., |(a, ..)| *a)
}
pub(in crate::app) const BANDS: [&str; 8] = [
    "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
];
fn band_color(i: usize) -> Color32 {
    [
        Color32::from_rgb(208, 69, 61),
        Color32::from_rgb(225, 137, 55),
        Color32::from_rgb(213, 193, 61),
        Color32::from_rgb(90, 160, 92),
        Color32::from_rgb(72, 178, 178),
        Color32::from_rgb(78, 125, 207),
        Color32::from_rgb(141, 103, 191),
        Color32::from_rgb(193, 92, 165),
    ][i]
}
/// Rail colors for a mixer slider: neighbouring hues for Hue, gray to color
/// for Saturation and dark to light for Luminance.
fn hsl_gradient(palette: &theme::Palette, band: usize, channel: usize) -> (Color32, Color32) {
    let color = band_color(band);
    match channel {
        0 => (band_color((band + 7) % 8), band_color((band + 1) % 8)),
        1 => (palette.tone(Tone::DISABLED), color),
        _ => (
            palette.tone(Tone::BAR),
            color.lerp_to_gamma(Color32::WHITE, 0.45),
        ),
    }
}
/// A panel's switch while its section is drawn; a click on it is stored after.
pub(in crate::app) struct PanelSwitch {
    panel: Panel,
    pub(in crate::app) state: PanelState,
}
impl PanelSwitch {
    pub(in crate::app) fn new(r: &Recipe, panel: Panel) -> Self {
        Self {
            panel,
            state: r.panels.state(panel),
        }
    }
    pub(in crate::app) fn finish(self, r: &mut Recipe) {
        r.panels.set(self.panel, self.state);
    }
}
/// Marks a slider row a Targeted Adjustment Tool drag is moving, more strongly the
/// larger its `share` of the drag.
fn highlight_targeted(ui: &egui::Ui, row: Rect, share: f32) {
    if share <= 0. {
        return;
    }
    let painter = ui.painter();
    painter.rect_filled(
        row,
        radius::CONTROL,
        Color32::from_white_alpha((6. + 16. * share) as u8),
    );
    painter.rect_filled(
        Rect::from_min_size(row.left_top(), Vec2::new(3., row.height())),
        1.5,
        theme::palette(ui.ctx())
            .tone(Tone::STRONG)
            .gamma_multiply(0.4 + 0.6 * share),
    );
}
/// The Targeted Adjustment Tool's shortcut ending in `key`, as the tooltips show it.
fn targeted_shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘⌥⇧{key}")
    } else {
        format!("Ctrl+Alt+Shift+{key}")
    }
}
/// Group caption (Tone, Presence…) starting where the slider rails start.
fn subheading(ui: &mut egui::Ui, text: &str) -> Rect {
    crate::app::ui::widgets::set_edit_context(ui, text);
    ui.add_space(space::M);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.), Sense::hover());
    ui.painter().text(
        rect.left_center() + Vec2::new(control_left(ui), 0.),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(text::SMALL),
        theme::palette(ui.ctx()).tone(Tone::LABEL),
    );
    rect
}
/// A labelled control row on the slider grid: caption right-aligned in the
/// label column, controls from the rail start (5 pt past it) to the right edge,
/// and the same height as a slider row. Text too long for the row, such as a lens
/// or profile name, is cut short: wider rows push the panel past the window.
fn control_row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let column = label_column(ui);
    let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.), Sense::hover());
    ui.painter().text(
        Pos2::new(row.left() + column - LABEL_GAP, row.center().y),
        egui::Align2::RIGHT_CENTER,
        label,
        egui::FontId::proportional(text::SMALL),
        theme::palette(ui.ctx()).tone(Tone::SOFT),
    );
    let rect = Rect::from_min_max(
        Pos2::new(row.left() + column + LABEL_GAP, row.top()),
        row.max,
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = space::XS;
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            add(ui)
        },
    )
    .inner
}
/// Defringe's Amount and hue range for Purple, then Green. Double-clicking a hue
/// resets it to that color's own default range.
pub(in crate::app) fn defringe_sliders(
    ui: &mut egui::Ui,
    step: &mut StepName,
    e: &mut crate::model::effects::Effects,
) {
    for (i, name, hue) in [(0, "Purple", [0.55, 0.9]), (1, "Green", [0.2, 0.5])] {
        ui.push_id(("defringe", i), |ui| {
            slider_with(
                ui,
                step,
                &format!("{name} Amount"),
                &mut e.defringe[i],
                0. ..=1.,
                0.,
                Some((20., 0)),
                None,
            );
            let defaults = crate::model::effects::DEFRINGE_RANGES[i];
            let [lo, hi] = &mut e.defringe_ranges[i];
            let colors = hue.map(|h| {
                let c = crate::develop::color::hue_rgb(h).map(|v| (v * 180.) as u8);
                Color32::from_rgb(c[0], c[1], c[2])
            });
            let low = slider_with(
                ui,
                step,
                &format!("{name} Hue"),
                lo,
                0. ..=(*hi - 0.1).max(0.),
                defaults[0],
                None,
                Some((colors[0], colors[1])),
            );
            let high = ui
                .push_id("hi", |ui| {
                    slider_with(
                        ui,
                        step,
                        "",
                        hi,
                        (*lo + 0.1).min(1.)..=1.,
                        defaults[1],
                        None,
                        Some((colors[0], colors[1])),
                    )
                })
                .inner;
            // One range, as in Lightroom: resetting either end resets both, so the
            // other end never keeps the default out of reach.
            if [low, high].contains(&SliderEvent::Reset) {
                [*lo, *hi] = defaults;
                // Named after the whole range, not the end the slider clamped.
                step.name(
                    format!("Defringe {name} Hue"),
                    format!("{:.0} / {:.0}", defaults[0] * 100., defaults[1] * 100.),
                );
            }
        });
    }
}
fn eyedropper_icon(painter: &egui::Painter, c: Pos2, strong: bool) {
    let color = theme::palette(painter.ctx()).tone(if strong { Tone::STRONG } else { Tone::LABEL });
    icons::paint_at(painter, Icon::Eyedropper, c, 14., color);
}
/// The line that opens the Remove and Masking drawers: both tools are new.
/// What the experimental label's tooltip says about a tool.
const UNMEASURED: &str = "Not yet measured against Lightroom. Spots and masks are saved apart \
     from the rest of the edit, so older RAWmakase releases open the photo without them.";
const RED_EYE_NOTE: &str = "Fitted to Camera Raw on synthetic eyes. Corrections are saved apart \
     from the rest of the edit, so older RAWmakase releases open the photo without them.";
fn experimental(ui: &mut egui::Ui, note: &str) {
    ui.horizontal(|ui| {
        ui.add_space(label_column(ui));
        ui.label(
            egui::RichText::new("Experimental · early version")
                .size(text::MICRO)
                .color(theme::palette(ui.ctx()).warning()),
        )
        .on_hover_text(note);
    });
}
/// The tools whose drawers open in place of Develop's adjustments, as the
/// rail lists them, with their tooltips and the drawer's caption. Red Eye
/// has no shortcut, as in Lightroom.
pub(in crate::app) const DRAWER_TOOLS: [(Tool, &str, &str); 4] = [
    (Tool::Crop, "Crop & Straighten · R", "Crop & Straighten"),
    (
        Tool::Remove,
        "Spot Removal: Heal and Clone · Q",
        "Spot Removal",
    ),
    (Tool::RedEye, "Red Eye Correction", "Red Eye"),
    (Tool::Mask, "Masking · Shift+W", "Masking"),
];
/// The slider grid's label column in the tool drawers, wide enough for
/// their longest captions ("Orientation", "Range High").
pub(in crate::app) const DRAWER_LABEL_COLUMN: f32 = 68.;
/// The height of the Crop drawer's controls, centred in their 26 pt rows.
const CROP_CONTROL: f32 = 22.;
/// The Crop aspect's padlock, a button: closed while drags keep the crop's aspect,
/// open while it is Free. Clicking it, or A, locks or unlocks it.
fn aspect_lock(ui: &mut egui::Ui, locked: bool) -> egui::Response {
    let (icon, label, hint) = if locked {
        (Icon::AspectLocked, "Aspect locked", "Unlock aspect · A")
    } else {
        (Icon::AspectUnlocked, "Aspect unlocked", "Lock aspect · A")
    };
    // Selected while locked, as the Ruler button is while armed: the shackle alone
    // is too small a difference to see.
    let response = ui.add_sized(
        [CROP_CONTROL, CROP_CONTROL],
        egui::Button::new("").selected(locked),
    );
    let ink = if locked {
        ui.visuals().selection.stroke.color
    } else {
        ui.style().interact(&response).fg_stroke.color
    };
    icons::paint_at(ui.painter(), icon, response.rect.center(), 14., ink);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    response.on_hover_text(hint)
}
/// A Crop drawer button showing `icon`, `width` wide.
fn crop_icon_button(ui: &mut egui::Ui, icon: Icon, width: f32) -> egui::Response {
    let response = ui.add_sized([width, CROP_CONTROL], egui::Button::new(""));
    let ink = ui.style().interact(&response).fg_stroke.color;
    icons::paint_at(ui.painter(), icon, response.rect.center(), 14., ink);
    response
}
/// A tool drawer's contents, at the adjustments sections' margins.
fn drawer_frame() -> egui::Frame {
    egui::Frame::new().inner_margin(egui::Margin {
        left: inset::PANEL as i8,
        right: inset::PANEL as i8,
        top: space::M as i8,
        bottom: space::L as i8,
    })
}
/// Whether `tool` has a drawer in place of the adjustments.
pub(in crate::app) fn has_drawer(tool: Tool) -> bool {
    DRAWER_TOOLS.iter().any(|(t, ..)| *t == tool)
}
/// A drawer tool's icon on the rail, a Lucide outline as its tabs' are.
pub(in crate::app) fn tool_icon(tool: Tool) -> Icon {
    match tool {
        Tool::Crop => Icon::Crop,
        Tool::Remove => Icon::Bandage,
        Tool::RedEye => Icon::Eye,
        _ => Icon::MaskTool,
    }
}

/// A row starting at the slider rails, returning what `add` does.
fn rail_row<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.add_space(control_left(ui) - ui.spacing().item_spacing.x);
        add(ui)
    })
    .inner
}

/// [`setting_control`] for a Basic slider, which the `.` and `,` keys select for
/// `+` and `-` to step (`basic_keys`): its label lights up while it is the
/// `selected` one, and a click on the label selects it.
fn basic_control(
    ui: &mut egui::Ui,
    step: &mut StepName,
    r: &mut Recipe,
    id: ParameterId,
    default: f32,
    photo: Option<&crate::camera_data::Metadata>,
    selected: &mut Option<ParameterId>,
) {
    let canonical = basic_keys::canonical(id);
    let lit = *selected == Some(canonical) && basic_keys::enabled(id, photo);
    let event = crate::model::edit::change_setting(r, id, photo, |r| {
        keyed_setting_slider(ui, step, id, id.value_mut(r), default, lit)
    });
    if event == SliderEvent::LabelClicked {
        *selected = Some(canonical);
        ui.ctx().request_repaint();
    }
}
/// The slider for develop setting `id`, and what changing it implies for the rest
/// of the recipe (`model::edit`).
fn setting_control(
    ui: &mut egui::Ui,
    step: &mut StepName,
    r: &mut Recipe,
    id: ParameterId,
    default: f32,
    photo: Option<&crate::camera_data::Metadata>,
) {
    crate::model::edit::change_setting(r, id, photo, |r| {
        setting_slider(ui, step, id, id.value_mut(r), default)
    });
}

#[cfg(test)]
#[path = "inspector/editor_tests.rs"]
mod editor_tests;
