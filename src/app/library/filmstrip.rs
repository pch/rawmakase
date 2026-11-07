//! Lightroom's filmstrip: one strip across the bottom of the window, below
//! both side panels, in every Library view and in Develop. It keeps its
//! width and scroll position as the views change; what a click does is up
//! to the view shown.
use super::grid::filter_caption;
use super::selection::{Mark, Selection};
use super::views::View;
use super::{Action, Library, cell};
use crate::app::ui::design::{Tone, inset, radius, space, text};
use crate::app::ui::theme;
use crate::catalog::{Photo, PhotoId};
use eframe::egui::{self, Color32, Vec2};

/// The strip's height, the same in every view.
pub(crate) const HEIGHT: f32 = 128.;
/// The id of the strip's panel; its scroll area is salted the same way.
/// Every view draws the one panel, so the strip keeps its scroll position.
pub(super) const ID: &str = "filmstrip";
/// A card's inset in its cell: cards are twice this apart, and the strip
/// pads its ends by it so the first and last are as far from its edges.
const CARD_INSET: f32 = space::XXS;

/// The module the strip is shown in. Both mark the whole selection; the Library lets
/// the view shown take a click, and Develop marks the photo it has open as active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Module {
    Library,
    Develop,
}

/// What happened in the strip this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Outcome {
    pub pick: Option<Pick>,
    /// A rating, flag or label was changed from the strip.
    pub metadata_changed: bool,
}

/// The strip's own state, beyond the scroll offset egui keeps.
#[derive(Debug, Default)]
pub(super) struct State {
    /// The photo last brought into view, and where it was then in
    /// `visible`: the strip scrolls again only when either changes.
    revealed: Option<(PhotoId, usize)>,
    /// The selection and `shown_version` the strip was last drawn with, to
    /// notice a view drawn after it changing either.
    drawn: (Selection, u64),
    /// The strip's glide after a trackpad swipe.
    kinetic: super::kinetic::Kinetic,
}

/// Draws the frame again before it is shown, as egui allows once a frame
/// (else on the next frame): for a change made after the strip was drawn.
pub(crate) fn redraw(ctx: &egui::Context, reason: &'static str) {
    ctx.request_discard(reason);
    if !ctx.will_discard() {
        ctx.request_repaint();
    }
}

/// The cells a strip scrolled to `viewport` shows, of `count` cells `width`
/// wide, and whether the viewport starts past them: an offset left from a
/// longer list, which the scroll area clamps only after this frame.
pub(super) fn in_view(
    viewport: egui::Rect,
    width: f32,
    count: usize,
) -> (std::ops::Range<usize>, bool) {
    let first = (viewport.min.x / width).floor().max(0.) as usize;
    let last = ((viewport.max.x / width).ceil().max(0.) as usize).min(count);
    (first.min(last)..last, count > 0 && first >= count)
}

/// A photo chosen in the filmstrip: clicked, or opened from its menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Pick {
    Show(PhotoId),
    Develop(PhotoId),
    /// Develop's Set as Reference Photo.
    Reference(PhotoId),
}
/// A catalog photo dragged from Develop's filmstrip, e.g. onto Reference View.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DraggedPhoto(pub PhotoId);

impl Library {
    /// The Library's strip: brings the views up to date, draws the strip
    /// with the active photo, and carries out a click as the view shown
    /// takes it. Call before the side panels, so it spans the window.
    pub(crate) fn library_filmstrip(&mut self, ui: &mut egui::Ui) -> Action {
        self.prepare(ui.ctx());
        let active = self.selection.active;
        match self.filmstrip_panel(ui, active, Module::Library).pick {
            Some(pick) => self.filmstrip_pick(pick, ui.input(|i| i.modifiers)),
            None => Action::None,
        }
    }
    /// The filmstrip's panel, at the bottom of the window. One panel and one
    /// scroll area serve every view, so the strip stays where it was when
    /// they change. `current` is the photo shown: Develop's, or the active
    /// one.
    pub(crate) fn filmstrip_panel(
        &mut self,
        ui: &mut egui::Ui,
        current: Option<PhotoId>,
        module: Module,
    ) -> Outcome {
        egui::Panel::bottom(ID)
            .exact_size(HEIGHT)
            .frame(egui::Frame::new().fill(theme::palette(ui.ctx()).tone(Tone::BAR)))
            .show(ui, |ui| self.filmstrip(ui, current, module))
            .inner
    }
    /// Whether the selection changed after the strip was drawn, as a click
    /// in the grid below does: the strip then needs another frame to mark
    /// it and bring it into view.
    pub(crate) fn filmstrip_behind(&self) -> bool {
        self.strip.drawn.0 != self.selection || self.strip.drawn.1 != self.shown_version
    }
    /// A filmstrip click in Develop with Cmd or Shift: the photo joins or leaves the
    /// selection, or a range does, as in the grid, while the photo open stays active.
    /// Returns whether it was taken here; a plain click opens the photo instead.
    pub(crate) fn develop_select(
        &mut self,
        id: PhotoId,
        open: Option<PhotoId>,
        modifiers: egui::Modifiers,
    ) -> bool {
        if !modifiers.command && !modifiers.shift {
            return false;
        }
        // The open photo is the active one, stays in the selection, and is where a
        // Shift range starts.
        if let Some(open) = open {
            self.selection.active = Some(open);
            self.selection.anchor = Some(open);
            self.selection.selected.insert(open);
        }
        self.click(id, modifiers);
        if let Some(open) = open {
            self.selection.selected.insert(open);
            self.selection.active = Some(open);
            self.selection.anchor = Some(open);
        }
        true
    }
    /// Lightroom's Cmd+A in Develop's filmstrip: selects every shown photo, so Sync
    /// reaches them all, keeping the open photo active.
    pub(crate) fn develop_select_all(&mut self, open: Option<PhotoId>) {
        self.select_all();
        if let Some(open) = open {
            self.selection.selected.insert(open);
            self.selection.active = Some(open);
            self.selection.anchor = Some(open);
        }
    }
    /// A filmstrip click in the Library, as the view shown takes it: Grid,
    /// Loupe and Survey select as the grid does (Cmd and Shift add), Compare
    /// makes the photo its candidate, and Select activates its side.
    pub(super) fn filmstrip_pick(&mut self, pick: Pick, modifiers: egui::Modifiers) -> Action {
        let id = match pick {
            Pick::Develop(id) => return Action::Develop(id),
            Pick::Show(id) => id,
            Pick::Reference(_) => return Action::None,
        };
        match self.view() {
            View::Compare => self.compare_pick(id),
            View::Grid | View::Loupe | View::Survey => {
                self.click(id, modifiers);
                // The grid follows a photo chosen below it.
                self.scroll_to_active = true;
            }
        }
        Action::None
    }
    /// The photos in the current source, with `current` highlighted and,
    /// in the Library, the rest of the selection marked. With no photo
    /// current the strip still shows them. Returns a photo chosen and
    /// whether metadata changed.
    pub(super) fn filmstrip(
        &mut self,
        ui: &mut egui::Ui,
        current: Option<PhotoId>,
        module: Module,
    ) -> Outcome {
        let palette = theme::palette(ui.ctx());
        let mut target = None;
        let mut changed = false;
        let library = module == Module::Library;
        if self.filmstrip_behind() {
            self.strip.drawn = (self.selection.clone(), self.shown_version);
        }
        let photo = current.and_then(|id| self.photo(id)).cloned();
        let position = current.and_then(|current| {
            self.visible
                .iter()
                .position(|i| self.session.photos()[*i].id == current)
        });
        // The grid's edits cover its selection; elsewhere the photo shown.
        let whole_selection = library && self.view() == View::Grid;
        let mut remeasured = false;
        // In Develop on the photo area's margin, so its ends line up with the
        // toolbar's; in the Library with its view bar's.
        let side = if library {
            inset::BAR.left
        } else {
            crate::app::shell::workspace::PHOTO_MARGIN as i8
        };
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(side, space::XS as i8))
            .show(ui, |ui| {
                // As tall as the rating and label controls on the right, so
                // the captions placed before them centre on the same line,
                // and the cells below keep their size whether a photo is open
                // and the controls shown or not.
                ui.allocate_ui_with_layout(
                    Vec2::new(
                        ui.available_width(),
                        crate::app::library::photo_metadata::controls_height(ui),
                    ),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.label(
                            egui::RichText::new(self.source_name())
                                .size(text::SMALL)
                                .color(palette.tone(Tone::SOFT)),
                        );
                        let selected = self.selection.selected.len();
                        ui.label(filter_caption(
                            &palette,
                            &match position {
                                Some(_) if selected > 1 => {
                                    format!(
                                        "{} of {} photos selected",
                                        selected,
                                        self.visible.len()
                                    )
                                }
                                Some(at) => format!("{} of {} photos", at + 1, self.visible.len()),
                                None => format!("{} photos", self.visible.len()),
                            },
                        ));
                        if let Some(p) = &photo {
                            ui.label(filter_caption(
                                &palette,
                                &format!("{}{}", p.filename, cell::copy_suffix(p)),
                            ));
                            // Right-aligned by the width the controls took last frame, so
                            // the row never runs past the window and widens the strip.
                            let width_id = ui.id().with("controls-width");
                            let width = ui.data(|d| d.get_temp::<f32>(width_id)).unwrap_or(250.);
                            ui.add_space((ui.available_width() - width).max(space::M));
                            let start = ui.cursor().left();
                            changed = self.metadata_controls(ui, p.id, whole_selection);
                            let taken = ui.min_rect().right() - start;
                            if (taken - width).abs() > 0.5 {
                                ui.data_mut(|d| d.insert_temp(width_id, taken));
                                ui.ctx().request_repaint();
                                remeasured = true;
                            }
                        }
                    },
                );
            });
        // Scroll only to bring a newly shown photo into view, or one a sort
        // or filter moved: a photo already visible, e.g. one just clicked,
        // stays put, and so does a strip scrolled away from it.
        let shown = current.zip(position);
        let reveal = if shown != self.strip.revealed {
            self.strip.revealed = shown;
            position
        } else {
            None
        };
        // The strip was laid out too wide this frame, so bring the photo into view
        // again on the next, at its right width.
        if remeasured {
            self.strip.revealed = None;
        }
        let height = ui.available_height().max(40.);
        let size = Vec2::new(height * 1.25, height);
        if reveal.is_some() {
            self.strip.kinetic.stop();
        }
        let glide = self.strip.kinetic.begin(ui);
        // A mouse wheel rolled over the strip scrolls it sideways, with no
        // Shift needed. Only the strip changes this setting.
        let only_direction =
            std::mem::replace(&mut ui.style_mut().always_scroll_the_only_direction, true);
        egui::ScrollArea::horizontal()
            .id_salt(ID)
            .auto_shrink(false)
            .show_viewport(ui, |ui, viewport| {
                // Only the cells in view are laid out and drawn, however
                // many photos the source has.
                ui.set_min_size(Vec2::new(
                    size.x * self.visible.len() as f32 + 2. * CARD_INSET,
                    height,
                ));
                let origin = ui.max_rect().min + Vec2::new(CARD_INSET, 0.);
                let at = |n: usize| {
                    egui::Rect::from_min_size(origin + Vec2::new(size.x * n as f32, 0.), size)
                };
                if let Some(n) = reveal {
                    let rect = at(n);
                    if !ui.clip_rect().contains_rect(rect) {
                        ui.scroll_to_rect(rect, None);
                    }
                }
                let (cells, past_end) = in_view(
                    viewport.translate(Vec2::new(-CARD_INSET, 0.)),
                    size.x,
                    self.visible.len(),
                );
                if past_end {
                    redraw(ui.ctx(), "filmstrip scrolled past a shorter list");
                }
                for n in cells {
                    let photo = self.session.photos()[self.visible[n]].clone();
                    let mark = if current == Some(photo.id) {
                        Mark::Active
                    } else if self.selection.selected.contains(&photo.id) {
                        Mark::Selected
                    } else {
                        Mark::None
                    };
                    let (pick, edited) =
                        self.strip_cell(ui, at(n), &photo, mark, whole_selection, module);
                    target = pick.or(target);
                    if edited {
                        // Filters may have changed the visible list.
                        changed = true;
                        break;
                    }
                }
            });
        ui.style_mut().always_scroll_the_only_direction = only_direction;
        self.strip.kinetic.end(ui, glide);
        Outcome {
            pick: target,
            metadata_changed: changed,
        }
    }
    /// One photo in the strip, with its menu. Returns a photo chosen and
    /// whether the menu changed its metadata or stacks, and so maybe the
    /// visible list.
    fn strip_cell(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        photo: &Photo,
        mark: Mark,
        whole_selection: bool,
        module: Module,
    ) -> (Option<Pick>, bool) {
        // In Develop a photo can be dragged onto Reference View.
        let sense = match module {
            Module::Library => egui::Sense::click(),
            Module::Develop => egui::Sense::click_and_drag(),
        };
        let response = ui.interact(rect, ui.id().with(photo.id), sense);
        self.request_previews(photo, ui.ctx());
        // One palette read per cell, shared by everything the cell paints.
        let palette = theme::palette(ui.ctx());
        let image = paint_cell(
            ui.painter(),
            &palette,
            rect,
            photo,
            self.texture(photo),
            mark,
            response.hovered(),
        );
        // The badge collapses or expands the stack, as in the grid.
        let mut badge_clicked = false;
        if let Some(place) = photo.stack
            && self.filters.shows_stacks()
        {
            let badge = cell::stack_badge(ui.painter(), &palette, image.left_top(), place);
            badge_clicked = ui
                .interact(
                    badge,
                    egui::Id::new(("strip-stack-badge", photo.id)),
                    egui::Sense::click(),
                )
                .on_hover_text(if place.collapsed {
                    "Expand Stack"
                } else {
                    "Collapse Stack"
                })
                .clicked();
        }
        if module == Module::Develop {
            self.drag_source(ui, &response, photo);
        }
        let available = self.is_available(&photo.path);
        let single = self.menu_single(photo.id, whole_selection);
        let stacks = self.filters.shows_stacks();
        let presence = cell::Presence {
            available,
            editable_preview: self.has_editable_preview(photo.id),
        };
        let menu = cell::photo_menu(&response, photo, presence, single, module, stacks)
            .or(badge_clicked.then_some(cell::PhotoAction::StackBadge));
        if let Some(menu) = menu {
            if matches!(menu, cell::PhotoAction::SetReference) {
                return (Some(Pick::Reference(photo.id)), false);
            }
            let edited = matches!(
                menu,
                cell::PhotoAction::Edit(_)
                    | cell::PhotoAction::Stack(_)
                    | cell::PhotoAction::StackBadge
            );
            let develop = self.photo_action(ui.ctx(), photo, menu, whole_selection);
            return (develop.map(Pick::Develop), edited);
        }
        let context = crate::app::ui::widgets::context_clicked(&response);
        let clicked = response
            .on_hover_text(format!("{}{}", photo.filename, cell::copy_suffix(photo)))
            .clicked();
        ((clicked && !context).then_some(Pick::Show(photo.id)), false)
    }
}

impl Library {
    /// Lets a strip cell be dragged, its preview following the pointer.
    fn drag_source(&self, ui: &egui::Ui, response: &egui::Response, photo: &Photo) {
        response.dnd_set_drag_payload(DraggedPhoto(photo.id));
        if !response.dragged() {
            return;
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let (Some(pos), Some(texture)) = (ui.ctx().pointer_latest_pos(), self.texture(photo))
        else {
            return;
        };
        let size = texture.size_vec2();
        let size = size * (96. / size.x.max(size.y));
        let layer = egui::LayerId::new(egui::Order::Tooltip, ui.id().with("dragged-photo"));
        let rect = egui::Rect::from_min_size(pos + Vec2::splat(8.), size);
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.));
        ui.ctx()
            .layer_painter(layer)
            .image(texture.id(), rect, uv, Color32::from_white_alpha(220));
    }
}

/// A strip cell: the preview over a row for its flag and stars, tinted by
/// its label, with the same cues as the grid. Returns where the preview is,
/// or the space it will fill while there is none.
fn paint_cell(
    painter: &egui::Painter,
    palette: &theme::Palette,
    rect: egui::Rect,
    photo: &Photo,
    texture: Option<&egui::TextureHandle>,
    mark: Mark,
    hovered: bool,
) -> egui::Rect {
    let active = mark == Mark::Active;
    let cell = rect.shrink(CARD_INSET);
    let base = palette.tone(match mark {
        Mark::Active => Tone::FAINT,
        Mark::Selected => Tone::OUTLINE_STRONG,
        Mark::None if hovered => Tone::HOVER,
        Mark::None => Tone::FIELD,
    });
    let fill = crate::app::library::photo_metadata::label_color(palette, &photo.label)
        .map_or(base, |label| {
            base.lerp_to_gamma(label, if mark == Mark::None { 0.25 } else { 0.35 })
        });
    painter.rect_filled(cell, radius::SMALL, fill);
    let strip = 14.;
    let area = egui::Rect::from_min_max(
        cell.min + Vec2::splat(5.),
        cell.max - Vec2::new(5., strip + 2.),
    );
    let mut drawn = area;
    if let Some(texture) = texture {
        let size = texture.size_vec2();
        let scale = (area.width() / size.x).min(area.height() / size.y);
        let image = egui::Rect::from_center_size(area.center(), size * scale);
        painter.image(
            texture.id(),
            image,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
            Color32::WHITE,
        );
        if photo.master.is_some() {
            cell::copy_badge(painter, palette, image, fill);
        }
        drawn = image;
    }
    let y = cell.bottom() - strip / 2. - 2.;
    let mut x = cell.left() + 6.;
    if photo.flag != 0 {
        crate::app::library::photo_metadata::flag_icon(
            painter,
            palette,
            egui::pos2(x + 4., y),
            photo.flag,
            active,
        );
        x += 13.;
    }
    if photo.rating > 0 {
        painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            "★".repeat(photo.rating as usize),
            egui::FontId::proportional(text::MICRO),
            palette.tone(if active { Tone::CARD } else { Tone::SOFT }),
        );
    }
    drawn
}
