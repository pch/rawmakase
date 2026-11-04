use crate::app::ui::design::{Tone, radius, text};
use crate::app::ui::theme;
use crate::catalog::Photo;
use eframe::egui::{self, Color32, Vec2};

/// Lightroom's grid cell styles, cycled with J: compact cells with their
/// number, name, flag and rating; expanded cells, which add a line of file
/// details; or the photos alone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Style {
    #[default]
    Compact,
    Expanded,
    Plain,
}
impl Style {
    pub(super) const ALL: [Self; 3] = [Self::Compact, Self::Expanded, Self::Plain];
    pub(super) fn next(self) -> Self {
        match self {
            Self::Compact => Self::Expanded,
            Self::Expanded => Self::Plain,
            Self::Plain => Self::Compact,
        }
    }
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Compact => "Compact Cells",
            Self::Expanded => "Expanded Cells",
            Self::Plain => "Photos Only",
        }
    }
    /// A cell's height for its `width`: expanded cells are taller by their
    /// details line.
    pub(super) fn height(self, width: f32) -> f32 {
        match self {
            Self::Expanded => width + DETAILS,
            Self::Compact | Self::Plain => width,
        }
    }
}
/// The height of an expanded cell's details line.
const DETAILS: f32 = 16.;

/// How a grid cell shows its photo.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Shown {
    pub mark: super::selection::Mark,
    /// Its place in the grid, from 1.
    pub number: usize,
    pub available: bool,
    /// In the Quick Collection.
    pub quick: bool,
    pub style: Style,
    /// File details for an expanded cell, e.g. "6000 × 4000 · 29/06/2016".
    pub details: String,
    /// Its menu acts on this photo alone, not a selection of several.
    pub single: bool,
    /// The source shows stacks (see `Filters::shows_stacks`).
    pub stacks: bool,
}
pub(super) fn photo_cell(
    ui: &mut egui::Ui,
    photo: &Photo,
    texture: Option<&egui::TextureHandle>,
    shown: Shown,
    width: f32,
) -> (egui::Response, Option<PhotoAction>) {
    let palette = theme::palette(ui.ctx());
    let Shown {
        mark,
        number,
        available,
        quick,
        style,
        details,
        single,
        stacks,
    } = shown;
    let stack = photo.stack.filter(|_| stacks);
    use crate::app::library::photo_metadata::label_color;
    use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, StrokeKind};
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, style.height(width)), Sense::hover());
    let extras = style != Style::Plain;
    // Selection reveals metadata in the side panel. A stable ID keeps the
    // first right-click menu open when that changes the surrounding widget tree.
    let response = ui.interact(
        rect,
        egui::Id::new(("catalog-photo-cell", photo.id)),
        // Dragged onto a folder, the photos move there.
        Sense::click_and_drag(),
    );
    // Lightroom grid: dark cells separated by thin gutters, a light surround
    // for the selection, and the color label tinting the cell.
    let cell = rect.shrink(1.);
    let selected = mark != super::selection::Mark::None;
    let active = mark == super::selection::Mark::Active;
    // The active photo is lighter than the rest of the selection.
    let base = if active {
        palette.tone(Tone::SECONDARY)
    } else if selected {
        palette.tone(Tone::DISABLED)
    } else if response.hovered() {
        palette.tone(Tone::PRESSED)
    } else {
        palette.tone(Tone::OUTLINE)
    };
    let fill = label_color(&palette, &photo.label).map_or(base, |label| {
        base.lerp_to_gamma(label, if selected { 0.35 } else { 0.22 })
    });
    let painter = ui.painter();
    painter.rect_filled(cell, radius::SMALL, fill);
    painter.rect_stroke(
        cell,
        1.,
        Stroke::new(
            1.,
            palette.tone(match mark {
                super::selection::Mark::Active => Tone::SOFT,
                super::selection::Mark::Selected => Tone::SECONDARY,
                super::selection::Mark::None => Tone::FIELD,
            }),
        ),
        StrokeKind::Inside,
    );
    let ink = palette.tone(if selected { Tone::HOVER } else { Tone::FAINT });
    let header = if extras {
        (width * 0.13).clamp(16., 26.)
    } else {
        4.
    };
    if extras {
        painter.text(
            cell.left_top() + Vec2::new(6., 3.),
            Align2::LEFT_TOP,
            number.to_string(),
            FontId::proportional(header * 0.95),
            palette.tone(if selected {
                Tone::FAINT
            } else {
                Tone::OUTLINE_STRONG
            }),
        );
    }
    // An expanded cell's details, under its number and name.
    let header = if style == Style::Expanded {
        let at = Pos2::new(cell.left() + 7., cell.top() + header + 2.);
        painter
            .with_clip_rect(Rect::from_min_size(
                at,
                Vec2::new(cell.width() - 14., DETAILS),
            ))
            .text(
                at,
                Align2::LEFT_TOP,
                details,
                FontId::proportional(text::MICRO),
                ink,
            );
        header + DETAILS
    } else {
        header
    };
    if extras && width >= 140. {
        let name = painter.layout_no_wrap(
            photo.filename.clone(),
            FontId::proportional(text::MICRO),
            ink,
        );
        let space = (cell.width() * 0.62).min(name.size().x);
        painter
            .with_clip_rect(Rect::from_min_size(
                Pos2::new(cell.right() - 7. - space, cell.top() + 5.),
                Vec2::new(space, 14.),
            ))
            .galley(
                Pos2::new(cell.right() - 7. - space, cell.top() + 5.),
                name,
                ink,
            );
    }
    let footer = if extras { 20. } else { 4. };
    let margin = (width * 0.08).max(8.);
    let area = Rect::from_min_max(
        Pos2::new(cell.left() + margin, cell.top() + header + 4.),
        Pos2::new(cell.right() - margin, cell.bottom() - footer - 2.),
    );
    if let Some(texture) = texture {
        let size = texture.size_vec2();
        let scale = (area.width() / size.x).min(area.height() / size.y);
        let image = Rect::from_center_size(area.center(), size * scale);
        painter.rect_filled(
            image.expand(1.).translate(Vec2::new(1.5, 2.)),
            0.,
            Color32::from_black_alpha(90),
        );
        painter.image(
            texture.id(),
            image,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
            Color32::WHITE,
        );
        painter.rect_stroke(
            image,
            0.,
            Stroke::new(1., Color32::from_black_alpha(160)),
            StrokeKind::Outside,
        );
        if photo.master.is_some() {
            copy_badge(painter, &palette, image, fill);
        }
    }
    // Lightroom's stack badge, on the photo's upper left corner: a click
    // collapses or expands the stack.
    let mut badge_clicked = false;
    if let Some(place) = stack {
        let corner = texture.map_or(area.left_top(), |t| {
            let size = t.size_vec2();
            let scale = (area.width() / size.x).min(area.height() / size.y);
            Rect::from_center_size(area.center(), size * scale).left_top()
        });
        let badge = stack_badge(painter, &palette, corner, place);
        badge_clicked = ui
            .interact(
                badge,
                egui::Id::new(("catalog-stack-badge", photo.id)),
                Sense::click(),
            )
            .on_hover_text(if place.collapsed {
                "Expand Stack (S)"
            } else {
                "Collapse Stack (S)"
            })
            .clicked();
    }
    if texture.is_none() {
        painter.text(
            area.center(),
            Align2::CENTER_CENTER,
            if available { &photo.format } else { "Offline" },
            FontId::proportional(text::SMALL),
            ink,
        );
    }
    if extras {
        let badges = Badges {
            quick,
            selected,
            available,
            ink,
        };
        footer_badges(painter, &palette, cell, footer, photo, badges);
    }
    let action = photo_menu(
        &response,
        photo,
        available,
        single,
        super::Module::Library,
        stacks,
    );
    let action = action.or(badge_clicked.then_some(PhotoAction::StackBadge));
    (response, action)
}
/// What a cell's footer shows besides the photo's own flag and rating.
struct Badges {
    quick: bool,
    selected: bool,
    available: bool,
    /// The colour of the cell's text.
    ink: Color32,
}
/// The footer of a cell with extras: flag, stars, copy name, the Quick
/// Collection marker and the offline mark.
fn footer_badges(
    painter: &egui::Painter,
    palette: &theme::Palette,
    cell: egui::Rect,
    footer: f32,
    photo: &Photo,
    badges: Badges,
) {
    use crate::app::library::photo_metadata::flag_icon;
    use egui::{Align2, FontId, Pos2, Rect};
    let Badges {
        quick,
        selected,
        available,
        ink,
    } = badges;
    // Lightroom's Quick Collection marker, in the footer's right corner,
    // clear of the file name.
    if quick {
        painter.circle_filled(
            Pos2::new(cell.right() - 10., cell.bottom() - footer / 2. - 1.),
            3.5,
            palette.tone(if selected { Tone::FIELD } else { Tone::TEXT }),
        );
    }
    let y = cell.bottom() - footer / 2. - 1.;
    let mut x = cell.left() + 7.;
    if photo.flag != 0 {
        flag_icon(painter, palette, Pos2::new(x + 4., y), photo.flag, selected);
        x += 14.;
    }
    if photo.rating > 0 {
        painter.text(
            Pos2::new(x, y),
            Align2::LEFT_CENTER,
            "★".repeat(photo.rating as usize),
            FontId::proportional(text::MICRO),
            palette.tone(if selected { Tone::PANEL } else { Tone::SOFT }),
        );
    }
    if photo.master.is_some() {
        let name = if photo.copy_name.is_empty() {
            "Copy"
        } else {
            &photo.copy_name
        };
        let galley = painter.layout_no_wrap(name.into(), FontId::proportional(text::MICRO), ink);
        let space = cell.width() * 0.36;
        let left = cell.center().x - galley.size().x.min(space) / 2.;
        painter
            .with_clip_rect(Rect::from_min_size(
                Pos2::new(left, y - 7.),
                Vec2::new(space, 14.),
            ))
            .galley(Pos2::new(left, y - galley.size().y / 2.), galley, ink);
    }
    if !available {
        painter.text(
            Pos2::new(cell.right() - 24., y),
            Align2::RIGHT_CENTER,
            "?",
            FontId::proportional(text::SMALL),
            Color32::from_rgb(210, 150, 60),
        );
    }
}
/// Lightroom's virtual copy badge: the image's lower left corner folded
/// over. `background` is what shows behind the fold.
pub(in crate::app) fn copy_badge(
    painter: &egui::Painter,
    palette: &theme::Palette,
    image: egui::Rect,
    background: Color32,
) {
    let size = (image.width().min(image.height()) * 0.12).clamp(8., 16.);
    let corner = image.left_bottom();
    let up = corner - Vec2::new(0., size);
    let right = corner + Vec2::new(size, 0.);
    painter.add(egui::Shape::convex_polygon(
        vec![corner, right, up],
        background,
        egui::Stroke::NONE,
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![up, right, corner + Vec2::new(size, -size)],
        palette.tone(Tone::TEXT),
        egui::Stroke::new(1., Color32::from_black_alpha(160)),
    ));
}
/// The stack badge at `corner`: the stack's count on its top, collapsed or
/// not, and each other photo's place in it while expanded. Returns where it
/// is, for a click.
pub(in crate::app) fn stack_badge(
    painter: &egui::Painter,
    palette: &theme::Palette,
    corner: egui::Pos2,
    place: crate::catalog::StackPlace,
) -> egui::Rect {
    let label = if place.is_top() {
        place.count.to_string()
    } else {
        format!("{} of {}", place.index + 1, place.count)
    };
    let galley = painter.layout_no_wrap(
        label,
        egui::FontId::proportional(text::MICRO),
        palette.tone(Tone::STRONG),
    );
    let lines = if place.is_top() { 7. } else { 0. };
    let size = galley.size() + Vec2::new(8. + lines, 2.);
    let badge = egui::Rect::from_min_size(corner + Vec2::splat(3.), size);
    painter.rect_filled(badge, radius::SMALL, Color32::from_black_alpha(170));
    if place.is_top() {
        // Two cards, one behind the other: a stack, collapsed or not.
        let card = egui::Rect::from_min_size(badge.min + Vec2::new(4., 3.5), Vec2::splat(5.));
        let ink = palette.tone(if place.collapsed {
            Tone::STRONG
        } else {
            Tone::SOFT
        });
        painter.rect_stroke(
            card.translate(Vec2::new(1.5, -1.5)),
            0.,
            egui::Stroke::new(1., ink),
            egui::StrokeKind::Inside,
        );
        painter.rect_filled(card, 0., ink);
    }
    painter.galley(
        badge.min + Vec2::new(4. + lines, 1.),
        galley,
        palette.tone(Tone::STRONG),
    );
    badge
}
/// " / Copy 1" for a virtual copy, as Lightroom names it after the file.
pub(in crate::app) fn copy_suffix(photo: &Photo) -> String {
    match photo.master {
        Some(_) if photo.copy_name.is_empty() => " / Copy".into(),
        Some(_) => format!(" / {}", photo.copy_name),
        None => String::new(),
    }
}
/// What a thumbnail's context menu asked for.
pub(in crate::app) enum PhotoAction {
    Develop,
    Reveal,
    CopyPath,
    /// Find on Disk…: pick the file of an offline photo.
    FindOnDisk,
    Edit(crate::app::library::photo_metadata::Edit),
    Copy(super::CopyAction),
    /// Read Metadata from Files.
    ReadMetadata,
    /// Remove from Catalog.
    Remove,
    /// Move to Trash.
    Trash,
    /// Rename Photo… or Rename Photos….
    Rename,
    /// Move to Folder…: the photo's files to another catalog folder.
    MoveToFolder,
    /// Export as Catalog…: the photos into a new catalog of their own.
    ExportCatalog,
    /// Library › Previews: Build Standard-Sized Previews.
    BuildPreviews(crate::catalog::preview_cache::PreviewKind),
    /// Library › Previews: Discard 1:1 Previews, or Standard and 1:1.
    DiscardPreviews(Vec<crate::catalog::preview_cache::PreviewKind>),
    /// Develop's Set as Reference Photo.
    SetReference,
    /// A Photo › Stacking command.
    Stack(super::StackAction),
    /// The stack badge was clicked: that stack collapses or expands.
    StackBadge,
}
/// Remove from Catalog's shortcut, as Lightroom shows it.
const REMOVE_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "⌥⌫"
} else {
    "Alt+Backspace"
};
/// Move to Trash's shortcut, as Lightroom shows Remove and Trash Photos'.
const TRASH_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "⌘⌥⇧⌫"
} else {
    "Ctrl+Alt+Shift+Backspace"
};
/// Create Virtual Copy's shortcut, as Lightroom shows it.
pub(in crate::app) const VIRTUAL_COPY_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "⌘'"
} else {
    "Ctrl+'"
};
/// Group into Stack's and Unstack's shortcuts, as Lightroom shows them.
const GROUP_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "⌘G"
} else {
    "Ctrl+G"
};
const UNSTACK_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "⇧⌘G"
} else {
    "Ctrl+Shift+G"
};
/// Photo › Stacking, for `photo`; the choice made.
fn stacking_menu(
    ui: &mut egui::Ui,
    photo: &Photo,
    module: super::Module,
) -> Option<super::StackAction> {
    use super::StackAction as A;
    use crate::app::ui::widgets::{menu_item, menu_separator};
    let stacked = photo.stack.is_some();
    let (top, last) = photo
        .stack
        .map_or((false, false), |s| (s.is_top(), s.index + 1 == s.count));
    let library = module == super::Module::Library;
    let toggle = if photo.stack.is_some_and(|s| s.collapsed) {
        "Expand Stack"
    } else {
        "Collapse Stack"
    };
    let items = [
        Some(("Group into Stack", GROUP_SHORTCUT, true, A::Group)),
        Some(("Unstack", UNSTACK_SHORTCUT, stacked, A::Unstack)),
        Some(("Remove from Stack", "", stacked, A::Remove)),
        None,
        Some(("Move to Top of Stack", "⇧S", stacked && !top, A::MoveToTop)),
        Some(("Move Up in Stack", "", stacked && !top, A::MoveUp)),
        Some(("Move Down in Stack", "", stacked && !last, A::MoveDown)),
        None,
        Some((toggle, "S", stacked, A::Toggle)),
        Some(("Collapse All Stacks", "", true, A::CollapseAll)),
        Some(("Expand All Stacks", "", true, A::ExpandAll)),
        None,
        Some(("Auto-Stack by Capture Time…", "", library, A::AutoStack)),
    ];
    let mut choice = None;
    for item in items {
        match item {
            None => menu_separator(ui),
            Some((title, shortcut, enabled, action)) => {
                if menu_item(ui, title, shortcut, enabled, false) {
                    choice = Some(action);
                }
            }
        }
    }
    choice
}
/// Whether Find on Disk… is offered: for one photo, offline.
pub(in crate::app) fn find_on_disk_enabled(available: bool, single: bool) -> bool {
    !available && single
}
/// The right-click menu shared by grid cells and the Develop filmstrip.
/// Open in Develop is greyed out for a photo Develop cannot open, e.g. one
/// not `available`, with the reason in place of its shortcut. In Develop's
/// filmstrip it starts with Set as Reference Photo, as in Lightroom. Find on
/// Disk… is offered for a photo not `available` when the menu acts on it
/// alone (`single`). Stacking is offered where the source shows stacks
/// (`stacks`), or for a photo in none.
pub(in crate::app) fn photo_menu(
    response: &egui::Response,
    photo: &Photo,
    available: bool,
    single: bool,
    module: super::Module,
    stacks: bool,
) -> Option<PhotoAction> {
    use crate::app::library::photo_metadata::{Edit, LABELS};
    use crate::app::ui::widgets::{menu_item, menu_separator, submenu_style};
    let mut action = None;
    // Why Develop can't open the photo reads its file: once each time the menu opens.
    let key = response.id.with(("develop-refusal", photo.id, available));
    if crate::app::ui::widgets::context_clicked(response) {
        response
            .ctx
            .data_mut(|d| d.remove::<Option<super::Refusal>>(key));
    }
    crate::app::ui::widgets::context_menu(response, |ui| {
        ui.set_width(210.);
        ui.spacing_mut().item_spacing.y = 0.;
        let refusal = ui.ctx().data_mut(|d| {
            d.get_temp_mut_or_insert_with(key, || super::develop_refusal(photo, available))
                .clone()
        });
        if module == super::Module::Develop {
            if menu_item(ui, "Set as Reference Photo", "", refusal.is_none(), false) {
                action = Some(PhotoAction::SetReference);
                ui.close();
            }
            menu_separator(ui);
        }
        let shortcut = refusal.as_ref().map_or("D".into(), super::Refusal::label);
        if menu_item(ui, "Open in Develop", &shortcut, refusal.is_none(), false) {
            action = Some(PhotoAction::Develop);
            ui.close();
        }
        if menu_item(ui, crate::platform::reveal::LABEL, "", true, false) {
            action = Some(PhotoAction::Reveal);
            ui.close();
        }
        if menu_item(ui, "Copy File Path", "", true, false) {
            action = Some(PhotoAction::CopyPath);
            ui.close();
        }
        if menu_item(
            ui,
            "Find on Disk…",
            "",
            find_on_disk_enabled(available, single),
            false,
        ) {
            action = Some(PhotoAction::FindOnDisk);
            ui.close();
        }
        menu_separator(ui);
        use super::CopyAction;
        let copy = photo.master.is_some();
        for (title, shortcut, enabled, choice) in [
            (
                "Create Virtual Copy",
                VIRTUAL_COPY_SHORTCUT,
                true,
                CopyAction::Create(photo.id),
            ),
            (
                "Set Copy as Master",
                "",
                copy,
                CopyAction::SetMaster(photo.id),
            ),
            (
                "Remove Virtual Copy…",
                "",
                copy,
                CopyAction::Remove(photo.id),
            ),
        ] {
            if menu_item(ui, title, shortcut, enabled, false) {
                action = Some(PhotoAction::Copy(choice));
                ui.close();
            }
        }
        menu_separator(ui);
        submenu_style(ui);
        ui.add_enabled_ui(stacks, |ui| {
            ui.menu_button("Stacking", |ui| {
                ui.set_width(250.);
                ui.spacing_mut().item_spacing.y = 0.;
                if let Some(choice) = stacking_menu(ui, photo, module) {
                    action = Some(PhotoAction::Stack(choice));
                    ui.close();
                }
            })
        });
        menu_separator(ui);
        if menu_item(ui, "Read Metadata from Files…", "", true, false) {
            action = Some(PhotoAction::ReadMetadata);
            ui.close();
        }
        if menu_item(ui, "Rename…", "F2", true, false) {
            action = Some(PhotoAction::Rename);
            ui.close();
        }
        if menu_item(ui, "Move to Folder…", "", true, false) {
            action = Some(PhotoAction::MoveToFolder);
            ui.close();
        }
        if menu_item(ui, "Export as Catalog…", "", true, false) {
            action = Some(PhotoAction::ExportCatalog);
            ui.close();
        }
        if menu_item(ui, "Remove from Catalog…", REMOVE_SHORTCUT, true, false) {
            action = Some(PhotoAction::Remove);
            ui.close();
        }
        if menu_item(ui, "Move to Trash…", TRASH_SHORTCUT, true, false) {
            action = Some(PhotoAction::Trash);
            ui.close();
        }
        menu_separator(ui);
        submenu_style(ui);
        ui.menu_button("Previews", |ui| {
            ui.set_width(250.);
            ui.spacing_mut().item_spacing.y = 0.;
            use crate::catalog::preview_cache::PreviewKind;
            if menu_item(ui, "Build Standard-Sized Previews", "", true, false) {
                action = Some(PhotoAction::BuildPreviews(PreviewKind::Standard));
                ui.close();
            }
            if menu_item(ui, "Build 1:1 Previews", "", true, false) {
                action = Some(PhotoAction::BuildPreviews(PreviewKind::OneToOne));
                ui.close();
            }
            menu_separator(ui);
            if menu_item(ui, "Discard 1:1 Previews", "", true, false) {
                action = Some(PhotoAction::DiscardPreviews(vec![PreviewKind::OneToOne]));
                ui.close();
            }
            if menu_item(ui, "Discard Standard and 1:1 Previews", "", true, false) {
                action = Some(PhotoAction::DiscardPreviews(PreviewKind::ALL.to_vec()));
                ui.close();
            }
        });
        menu_separator(ui);
        submenu_style(ui);
        ui.menu_button("Set Flag", |ui| {
            ui.set_width(170.);
            ui.spacing_mut().item_spacing.y = 0.;
            for (flag, title) in [(1, "Flagged"), (0, "Unflagged"), (-1, "Rejected")] {
                if menu_item(
                    ui,
                    title,
                    ["X", "U", "P"][(flag + 1) as usize],
                    true,
                    photo.flag == flag,
                ) {
                    action = Some(PhotoAction::Edit(Edit::Flag(flag)));
                    ui.close();
                }
            }
        });
        ui.menu_button("Set Rating", |ui| {
            ui.set_width(170.);
            ui.spacing_mut().item_spacing.y = 0.;
            for rating in 0..=5 {
                let title = if rating == 0 {
                    "None".into()
                } else {
                    "★".repeat(rating as usize)
                };
                if menu_item(
                    ui,
                    &title,
                    &rating.to_string(),
                    true,
                    photo.rating == rating,
                ) {
                    action = Some(PhotoAction::Edit(Edit::Rating(rating)));
                    ui.close();
                }
            }
        });
        ui.menu_button("Set Color Label", |ui| {
            let palette = theme::palette(ui.ctx());
            ui.set_width(170.);
            ui.spacing_mut().item_spacing.y = 0.;
            for (label, key) in LABELS
                .into_iter()
                .zip(["6", "7", "8", "9", ""])
                .chain([("", "")])
            {
                let title = if label.is_empty() { "None" } else { label };
                let before = ui.cursor().min;
                if menu_item(ui, title, key, true, photo.label == label) {
                    action = Some(PhotoAction::Edit(Edit::Label(label.into())));
                    ui.close();
                }
                if let Some(color) =
                    crate::app::library::photo_metadata::label_color(&palette, label)
                {
                    let chip = egui::Rect::from_center_size(
                        before + Vec2::new(ui.available_width() - 60., 12.),
                        Vec2::splat(9.),
                    );
                    ui.painter().rect_filled(chip, radius::SMALL, color);
                }
            }
        });
    });
    action
}
