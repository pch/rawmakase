//! Lightroom's sort orders for the grid and filmstrip. Photos come from the
//! catalog in capture order; another order sorts them by its key, stably,
//! so photos alike in it stay in capture order. Edit time and aspect ratio
//! are not on `Photo` and are read from the catalog as they are needed.
use crate::app::library::photo_metadata::LABELS;
use crate::catalog::{Catalog, Photo, PhotoId};
use std::cmp::Ordering;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Sort {
    #[default]
    CaptureTime,
    AddedOrder,
    EditTime,
    Rating,
    Pick,
    LabelColor,
    LabelText,
    FileName,
    Extension,
    AspectRatio,
}
impl Sort {
    pub(super) const ALL: [Self; 10] = [
        Self::CaptureTime,
        Self::AddedOrder,
        Self::EditTime,
        Self::Rating,
        Self::Pick,
        Self::LabelColor,
        Self::LabelText,
        Self::FileName,
        Self::Extension,
        Self::AspectRatio,
    ];
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::CaptureTime => "Capture Time",
            Self::AddedOrder => "Added Order",
            Self::EditTime => "Edit Time",
            Self::Rating => "Rating",
            Self::Pick => "Pick",
            Self::LabelColor => "Label Color",
            Self::LabelText => "Label Text",
            Self::FileName => "File Name",
            Self::Extension => "File Extension",
            Self::AspectRatio => "Aspect Ratio",
        }
    }
    /// Reads what this order needs beyond `Photo` from the catalog.
    pub(super) fn keys(self, catalog: &Catalog) -> Keys {
        match self {
            Self::EditTime => Keys::EditTimes(catalog.edit_times().unwrap_or_default()),
            Self::AspectRatio => Keys::Aspects(catalog.aspect_ratios().unwrap_or_default()),
            _ => Keys::None,
        }
    }
    /// Sorts `indices` into `photos`, which are in capture order, by this
    /// order, either way; photos alike in it stay in capture order.
    pub(super) fn sort(self, photos: &[Photo], indices: &mut [usize], keys: &Keys, reverse: bool) {
        if self == Self::CaptureTime {
            if reverse {
                indices.reverse();
            }
            return;
        }
        indices.sort_by(|a, b| {
            let (a, b) = (&photos[*a], &photos[*b]);
            let order = self.compare(a, b, keys);
            // Photos of unknown shape stay last either way.
            let unknown = |p: &Photo| matches!(keys, Keys::Aspects(k) if !k.contains_key(&p.id));
            let last = self == Self::AspectRatio && (unknown(a) || unknown(b));
            if reverse && !last {
                order.reverse()
            } else {
                order
            }
        });
    }
    fn compare(self, a: &Photo, b: &Photo, keys: &Keys) -> Ordering {
        match self {
            Self::CaptureTime => Ordering::Equal,
            Self::AddedOrder => a.id.cmp(&b.id),
            Self::EditTime => {
                let time = |p: &Photo| match keys {
                    Keys::EditTimes(times) => times.get(&p.id).map(String::as_str),
                    _ => None,
                };
                // Photos never edited come first, as the oldest.
                time(a).cmp(&time(b))
            }
            Self::Rating => a.rating.cmp(&b.rating),
            Self::Pick => a.flag.cmp(&b.flag),
            Self::LabelColor => label_rank(&a.label).cmp(&label_rank(&b.label)),
            Self::LabelText => caseless(&a.label, &b.label),
            Self::FileName => natural(&a.filename, &b.filename),
            Self::Extension => caseless(extension(&a.filename), extension(&b.filename)),
            Self::AspectRatio => {
                let aspect = |p: &Photo| match keys {
                    Keys::Aspects(aspects) => aspects.get(&p.id).copied(),
                    _ => None,
                };
                // Photos of unknown shape go last.
                match (aspect(a), aspect(b)) {
                    (Some(a), Some(b)) => a.total_cmp(&b),
                    (a, b) => a.is_none().cmp(&b.is_none()),
                }
            }
        }
    }
}

/// What an order needs beyond `Photo`, by photo.
#[derive(Debug, Default)]
pub(super) enum Keys {
    #[default]
    None,
    EditTimes(HashMap<PhotoId, String>),
    Aspects(HashMap<PhotoId, f32>),
}

/// Lightroom's label colour order: red, yellow, green, blue, purple, then
/// other labels, then none.
fn label_rank(label: &str) -> usize {
    match LABELS.iter().position(|l| *l == label) {
        Some(at) => at,
        None if label.is_empty() => LABELS.len() + 1,
        None => LABELS.len(),
    }
}
fn extension(filename: &str) -> &str {
    filename
        .rsplit_once('.')
        .map_or("", |(_, extension)| extension)
}
/// `a` against `b`, case aside, without allocating: sorting compares often.
fn caseless(a: &str, b: &str) -> Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
}
/// File names as people read them: case aside, and runs of digits by value,
/// so IMG_9 comes before IMG_10. Compares in place, without allocating.
fn natural(text_a: &str, text_b: &str) -> Ordering {
    let (a, b) = (text_a.as_bytes(), text_b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let ((x, next_i), (y, next_j)) = (digits(a, i), digits(b, j));
            let order = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
            if order != Ordering::Equal {
                return order;
            }
            (i, j) = (next_i, next_j);
        } else {
            // One character each, case aside; `i` and `j` stay on character
            // boundaries, as digits are one byte.
            let (x, y) = (text_a[i..].chars().next(), text_b[j..].chars().next());
            let (Some(x), Some(y)) = (x, y) else { break };
            let order = x.to_lowercase().cmp(y.to_lowercase());
            if order != Ordering::Equal {
                return order;
            }
            i += x.len_utf8();
            j += y.len_utf8();
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}
/// The run of digits in `s` from `from`, without its leading zeros, and
/// where it ends.
fn digits(s: &[u8], from: usize) -> (&[u8], usize) {
    let end = s[from..]
        .iter()
        .position(|c| !c.is_ascii_digit())
        .map_or(s.len(), |n| from + n);
    let start = s[from..end]
        .iter()
        .position(|c| *c != b'0')
        .map_or(end, |n| from + n);
    (&s[start..end], end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_sort_as_people_read_them() {
        assert_eq!(natural("Ärger.jpg", "ärger.jpg"), Ordering::Equal);
        assert_eq!(natural("Øst.jpg", "zebra.jpg"), Ordering::Greater);
        let mut names = [
            "img_10.jpg",
            "IMG_9.jpg",
            "img_010b.jpg",
            "a.jpg",
            "IMG_9.jpg",
        ];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(
            names,
            [
                "a.jpg",
                "IMG_9.jpg",
                "IMG_9.jpg",
                "img_10.jpg",
                "img_010b.jpg"
            ]
        );
    }

    #[test]
    fn labels_sort_in_lightrooms_colour_order() {
        let mut labels = ["", "Purple", "Client", "Red", "Green"];
        labels.sort_by_key(|l| label_rank(l));
        assert_eq!(labels, ["Red", "Green", "Purple", "Client", ""]);
    }
}
