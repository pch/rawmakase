//! When an editor shows a reduced draft of an edit before its full render,
//! as RAWmakase's Develop does: while a view is being edited, a draft keeps
//! sliders responsive on a machine slower than they change, unless the
//! view's last full render was quick, when a draft would only add work and a
//! blurry frame. Opening a photo or changing the view goes straight to the
//! full render.
//!
//! Each view (the whole photo, a region at 100%) keeps its own [`Drafts`].
use std::time::Duration;

/// A full render finished within this needs no draft before the next one.
pub const QUICK: Duration = Duration::from_millis(40);

/// A whole-photo draft halves the render's long edge until it has at most
/// this many pixels.
const DRAFT_PIXELS: f32 = 1_000_000.;

/// What one view knows of its last full render.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drafts {
    quick: bool,
}

impl Drafts {
    /// Whether the next render of the view starts with a draft: only while
    /// `editing` it (the same photo in the same view as the last frame shown)
    /// and only when its last full render was not quick.
    pub fn wanted(&self, editing: bool) -> bool {
        editing && !self.quick
    }
    /// Records the view's full render: it took `took`, and was `finished`,
    /// with nothing left to do before it shows. A frame read back from the GPU
    /// or still to be converted is not finished, so it never counts as quick.
    pub fn rendered(&mut self, took: Duration, finished: bool) {
        self.quick = finished && took < QUICK;
    }
}

/// The long edge of a whole-photo draft of a `width` by `height` render (after
/// crop, as a render's long edge applies to it), or `None` when the render is
/// small enough already.
pub fn draft_edge(width: u32, height: u32) -> Option<u32> {
    let aspect = width.min(height) as f32 / width.max(height).max(1) as f32;
    let mut edge = width.max(height) as f32;
    let mut halved = 0;
    while edge * edge * aspect > DRAFT_PIXELS && halved < 4 {
        edge /= 2.;
        halved += 1;
    }
    (halved > 0).then_some(edge.round() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_halves_the_edge_to_at_most_a_megapixel() {
        // A 3:2 photo's Fit on a large HiDPI view.
        assert_eq!(draft_edge(2360, 1573), Some(1180));
        assert_eq!(draft_edge(1200, 800), None);
        // Cropped square, the same long edge has more pixels.
        assert_eq!(draft_edge(1200, 1200), Some(600));
        // Four halvings at most.
        assert_eq!(draft_edge(20000, 13333), Some(1250));
    }

    #[test]
    fn a_draft_comes_first_while_editing_unless_the_last_render_was_quick() {
        let mut view = Drafts::default();
        assert!(!view.wanted(false), "a new photo or view: the full render");
        assert!(view.wanted(true));
        view.rendered(Duration::from_millis(10), true);
        assert!(!view.wanted(true));
        // Quick, but with finishing still to do: not quick.
        view.rendered(Duration::from_millis(10), false);
        assert!(view.wanted(true));
        view.rendered(QUICK, true);
        assert!(view.wanted(true));
    }
}
