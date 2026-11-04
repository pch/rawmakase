//! The Library's layout kept across launches: its view, filter bar, sort,
//! grid and whether folders show their subfolders' photos (see `app::preferences::session::LibraryLayout`). Each setting is saved under a
//! stable key, so renaming one in the interface keeps what was saved.
use super::Library;
use super::cell::Style;
use super::filter::{Kind, Label, RatingOp};
use super::sort::Sort;
use super::views::View;
use crate::app::preferences::session::LibraryLayout;

/// A setting saved by key.
trait Keyed: Copy + PartialEq + Default + 'static {
    const ALL: &'static [Self];
    fn key(self) -> &'static str;
    /// The setting saved as `key`; the default for one not known.
    fn from_key(key: &str) -> Self {
        Self::ALL
            .iter()
            .copied()
            .find(|v| v.key() == key)
            .unwrap_or_default()
    }
}
impl Keyed for View {
    const ALL: &'static [Self] = &[Self::Grid, Self::Loupe, Self::Compare, Self::Survey];
    fn key(self) -> &'static str {
        match self {
            Self::Grid => "grid",
            Self::Loupe => "loupe",
            Self::Compare => "compare",
            Self::Survey => "survey",
        }
    }
}
impl Keyed for Sort {
    const ALL: &'static [Self] = &Sort::ALL;
    fn key(self) -> &'static str {
        match self {
            Self::CaptureTime => "capture_time",
            Self::AddedOrder => "added_order",
            Self::EditTime => "edit_time",
            Self::Rating => "rating",
            Self::Pick => "pick",
            Self::LabelColor => "label_color",
            Self::LabelText => "label_text",
            Self::FileName => "file_name",
            Self::Extension => "extension",
            Self::AspectRatio => "aspect_ratio",
        }
    }
}
impl Keyed for Style {
    const ALL: &'static [Self] = &Style::ALL;
    fn key(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Expanded => "expanded",
            Self::Plain => "plain",
        }
    }
}
impl Keyed for RatingOp {
    const ALL: &'static [Self] = &RatingOp::ALL;
    fn key(self) -> &'static str {
        match self {
            Self::AtLeast => "at_least",
            Self::AtMost => "at_most",
            Self::Exactly => "exactly",
        }
    }
}
impl Keyed for Kind {
    const ALL: &'static [Self] = &[Self::All, Self::Masters, Self::Copies];
    fn key(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Masters => "masters",
            Self::Copies => "copies",
        }
    }
}
fn label_key(label: &Label) -> String {
    match label {
        Label::Color(name) => name.clone(),
        Label::None => "none".into(),
        Label::Other => "other".into(),
    }
}
fn label_from_key(key: &str) -> Label {
    match key {
        "none" => Label::None,
        "other" => Label::Other,
        name => Label::Color(name.into()),
    }
}

impl Library {
    /// How the Library shows its photos now, to keep.
    pub(crate) fn layout(&self) -> LibraryLayout {
        let filters = &self.filters;
        LibraryLayout {
            view: self.view().key().into(),
            sort: filters.sort.key().into(),
            reverse: filters.reverse,
            cell_style: self.cell_style.key().into(),
            thumb_size: Some(self.thumb_size),
            query: filters.query.clone(),
            flags: filters.flags.iter().copied().collect(),
            rating: filters.rating,
            rating_op: filters.rating_op.key().into(),
            labels: filters.labels.iter().map(label_key).collect(),
            kind: filters.kind.key().into(),
            filters_off: !filters.enabled,
            subfolders_hidden: !self.subfolders_shown,
        }
    }
    /// Returns to a layout kept from last time, after the source and
    /// selection are restored, so a Loupe, Compare or Survey has its photos.
    pub(crate) fn apply_layout(&mut self, layout: &LibraryLayout) {
        let filters = &mut self.filters;
        filters.sort = Sort::from_key(&layout.sort);
        filters.reverse = layout.reverse;
        filters.query = layout.query.clone();
        filters.flags = layout
            .flags
            .iter()
            .copied()
            .filter(|f| (-1..=1).contains(f))
            .collect();
        filters.rating = layout.rating.filter(|r| (0..=5).contains(r));
        filters.rating_op = RatingOp::from_key(&layout.rating_op);
        filters.labels = layout.labels.iter().map(|l| label_from_key(l)).collect();
        filters.kind = Kind::from_key(&layout.kind);
        filters.enabled = !layout.filters_off;
        self.cell_style = Style::from_key(&layout.cell_style);
        if let Some(size) = layout.thumb_size.filter(|s| (110. ..=360.).contains(s)) {
            self.thumb_size = size;
        }
        // The folders shown follow, and are filtered once with the rest.
        self.subfolders_shown = !layout.subfolders_hidden;
        if !self.selected_folders.is_empty() {
            self.filters.folder_scope = self.folders_scope(&self.selected_folders);
        }
        self.filter();
        // A view of one photo or more opens only on a photo still shown.
        let view = View::from_key(&layout.view);
        if view != View::Grid
            && self.selection.active.is_some_and(|id| {
                self.visible
                    .iter()
                    .any(|i| self.session.photos()[*i].id == id)
            })
        {
            self.set_view(view);
        }
    }
}
