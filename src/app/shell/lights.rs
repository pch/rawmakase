//! Lightroom's Lights Out: L dims everything but the photo, or the Library's
//! grid, then blacks it out, and on the third press brings the lights back.
//! Escape brings them back too. The shade is paint alone, on a layer just
//! above the workspace's, so clicks still reach the panels under it, as in
//! Lightroom, and windows, menus and messages show over it. Not saved: every
//! launch starts with the lights on.
use eframe::egui::{self, Color32, Rect};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum Lights {
    #[default]
    On,
    /// Everything but the photo under black at 80%.
    Dim,
    /// Everything but the photo black.
    Off,
}

impl Lights {
    /// What the next press of L gives.
    pub(in crate::app) fn next(self) -> Self {
        match self {
            Self::On => Self::Dim,
            Self::Dim => Self::Off,
            Self::Off => Self::On,
        }
    }
    fn shade(self) -> Option<Color32> {
        match self {
            Self::On => None,
            // Lightroom's default Dim Level.
            Self::Dim => Some(Color32::from_black_alpha(204)),
            Self::Off => Some(Color32::BLACK),
        }
    }
}

fn lit_id() -> egui::Id {
    egui::Id::new("lights-out-lit")
}

/// Keeps `rect` out of the shade this frame: the photo where it is drawn, or
/// the grid.
pub(in crate::app) fn light(ctx: &egui::Context, rect: Rect) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Vec<Rect>>(lit_id()).push(rect));
}

/// The parts of `screen` outside every rect of `lit`, as rects that don't
/// overlap: the screen cut into bands at the lit rects' top and bottom edges,
/// and each band into the spans no lit rect covers.
pub(in crate::app) fn shaded(screen: Rect, lit: &[Rect]) -> Vec<Rect> {
    let lit: Vec<Rect> = lit
        .iter()
        .map(|r| r.intersect(screen))
        .filter(|r| r.is_positive())
        .collect();
    let mut ys: Vec<f32> = [screen.top(), screen.bottom()]
        .into_iter()
        .chain(lit.iter().flat_map(|r| [r.top(), r.bottom()]))
        .collect();
    ys.sort_by(f32::total_cmp);
    ys.dedup();
    let mut out = Vec::new();
    for band in ys.windows(2) {
        let (top, bottom) = (band[0], band[1]);
        let mut spans: Vec<(f32, f32)> = lit
            .iter()
            .filter(|r| r.top() <= top && r.bottom() >= bottom)
            .map(|r| (r.left(), r.right()))
            .collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut x = screen.left();
        for (left, right) in spans {
            if left > x {
                out.push(Rect::from_x_y_ranges(x..=left, top..=bottom));
            }
            x = x.max(right);
        }
        if x < screen.right() {
            out.push(Rect::from_x_y_ranges(x..=screen.right(), top..=bottom));
        }
    }
    out
}

/// Shades the window for `lights`, all but what the views lit this frame
/// ([`light`]), or `fallback` when none did, and `also`. Call once a frame,
/// after the workspace is drawn: it also forgets what was lit.
pub(in crate::app) fn paint(
    ui: &egui::Ui,
    lights: Lights,
    fallback: Option<Rect>,
    also: Option<Rect>,
) {
    let ctx = ui.ctx();
    let mut lit = ctx
        .data_mut(|d| d.remove_temp::<Vec<Rect>>(lit_id()))
        .unwrap_or_default();
    let Some(color) = lights.shade() else {
        return;
    };
    if lit.is_empty() {
        lit.extend(fallback);
    }
    lit.extend(also);
    // Drawn right after the workspace's own layer, so under every window,
    // menu, message and tooltip, which have layers above it.
    let under = ui.layer_id();
    let layer = egui::LayerId::new(under.order, egui::Id::new("lights-out"));
    ctx.set_sublayer(under, layer);
    // One mesh, without the feathered edges that would show as seams where
    // two of its rects meet.
    let mut mesh = egui::Mesh::default();
    for rect in shaded(ctx.content_rect(), &lit) {
        mesh.add_colored_rect(rect, color);
    }
    ctx.layer_painter(layer).add(mesh);
}

#[cfg(test)]
#[path = "lights_editor_tests.rs"]
mod editor_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;

    fn area(rects: &[Rect]) -> f32 {
        rects.iter().map(|r| r.area()).sum()
    }
    /// Whether `a` and `b` share more than an edge.
    fn overlap(a: Rect, b: Rect) -> bool {
        a.intersect(b).is_positive()
    }

    #[test]
    fn l_cycles_dim_off_and_back_on() {
        assert_eq!(Lights::default(), Lights::On);
        assert_eq!(Lights::On.next(), Lights::Dim);
        assert_eq!(Lights::Dim.next(), Lights::Off);
        assert_eq!(Lights::Off.next(), Lights::On);
        assert_eq!(Lights::Dim.shade().map(|c| c.a()), Some(204));
        assert_eq!(Lights::Off.shade(), Some(Color32::BLACK));
        assert_eq!(Lights::On.shade(), None);
    }

    #[test]
    fn the_shade_covers_all_but_the_lit_rects_once() {
        let screen = Rect::from_min_max(pos2(0., 0.), pos2(1000., 800.));
        let photo = Rect::from_min_max(pos2(300., 100.), pos2(700., 600.));
        let line = Rect::from_min_max(pos2(400., 10.), pos2(600., 30.));
        let shade = shaded(screen, &[photo, line]);
        assert!((area(&shade) - (screen.area() - photo.area() - line.area())).abs() < 0.01);
        for (i, a) in shade.iter().enumerate() {
            assert!(!overlap(*a, photo) && !overlap(*a, line), "{a:?}");
            for b in &shade[..i] {
                assert!(!overlap(*a, *b), "{a:?} and {b:?} overlap");
            }
        }
        // A lit rect reaching past the window, and two overlapping ones.
        let wide = Rect::from_min_max(pos2(-50., 200.), pos2(1200., 300.));
        let shade = shaded(screen, &[wide, photo]);
        let lit = 1000. * 100. + photo.area() - 400. * 100.;
        assert!((area(&shade) - (screen.area() - lit)).abs() < 0.01);
        // Nothing lit: the whole window.
        assert_eq!(shaded(screen, &[]), vec![screen]);
    }
}
