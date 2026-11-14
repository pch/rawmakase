//! The sky's edge: the model's own answer everywhere, with an edge mode blended in only
//! where it does better than the model, by the kind of horizon the model sees there.
//!
//! A crown, bare or leafy, gets the [`sparse`](super::sparse) mode, which finds the sky between its
//! branches and twigs; a lattice the model is sure of (a bridge's truss, a fence) gets the
//! [`dense`](super::dense) mode, for the small gaps between its struts. Everything else keeps the
//! model's answer snapped to the photo's edges as it always was: a facade, a hillside, a
//! sea horizon, the open sky and its clouds. Water vetoes both modes, with priority: a
//! crown or reeds at a shore get the model's edge where they meet the water, since a
//! reflection or the water between reeds has the sky's own colours.
//!
//! The kinds come on the model's quarter grid. A cell counts for a kind only where the
//! model sees something other than sky there ([`OWNED`]); a crown's or a lattice's weight
//! reaches [`GATE`] into the sky around it, where its outermost twigs and gaps are, but
//! never onto a cell the model sees as something else; water's veto reaches further, past
//! a crown's reach and its easing, onto anything beside it. Both are then eased over
//! [`EASE`] and enlarged bilinearly, so a mode fades in and out over tens of pixels
//! instead of switching at a cell's edge.
//! The weights are not renormalised to one, only capped where a crown and a lattice
//! together exceed it: in open sky a few hundredths of a kind are not all the kind there
//! is.

use rawmakase_cancel::Cancel;
use std::time::{Duration, Instant};

use super::dense::dense_with;
use super::sparse::sparse_with;
use super::{GATE, cancelled, step};
use crate::kinds::Kinds;
use crate::process::{Rect, resample};
use crate::refine::{guided, mean, smooth};
use rawmakase_inference::InferenceError;

/// Share of a cell the model must see as something other than sky for the cell to
/// count for the kinds it sees there.
const OWNED: f32 = 0.3;
/// Vegetation's share of what the model sees in a cell, from which the sparse mode
/// starts to take over, to where it has taken over entirely.
const CROWN: (f32, f32) = (0.15, 0.5);
/// Lattice's share of what the model sees in a cell over which the dense mode takes
/// over: only where the model is sure of the lattice.
const SURE_LATTICE: (f32, f32) = (0.5, 0.8);
/// Water's share of what the model sees in a cell over which it vetoes the modes.
const VETO: (f32, f32) = (0.05, 0.25);
/// How far (fraction of the long side) the weights are eased.
const EASE: f32 = 0.006;
/// Below this weight anywhere in the photo the modes are not run.
const LEAST: f32 = 0.01;

/// Which edge modes [`blend`] ran for a photo, and how long they took with their
/// weighing and mixing: the time the sky's edge costs beyond the model's answer
/// snapped as main snaps it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Modes {
    pub sparse: bool,
    pub dense: bool,
    pub took: Duration,
}

/// Sky coverage (0..=1) from the model's answer, `p` (0..=1 at `width` x `height`), with
/// `rgb` and `luma` the photo's colours (sRGB / 255) and luma at the same size: the
/// model's answer snapped to the photo's edges, with the edge modes blended in by the
/// kinds of horizon in `kinds` (the same frame on its own grid). `cancel` is checked
/// before the edge is snapped, before each mode and between their passes, and before the
/// final mix. Also says which modes ran.
pub(crate) fn blend(
    rgb: &[[f32; 3]],
    luma: &[f32],
    p: &[f32],
    width: usize,
    height: usize,
    kinds: &Kinds,
    cancel: Cancel<'_>,
) -> Result<(Vec<f32>, Modes), InferenceError> {
    cancelled(cancel)?;
    let (w, h) = (width, height);
    let snapped = snapped(luma, p, w, h);
    let started = Instant::now();
    let (crowns, lattices) = weights(kinds, w, h);
    let busy = |weights: &[f32]| weights.iter().any(|v| *v > LEAST);
    let (by_crown, by_lattice) = (busy(&crowns), busy(&lattices));
    if !by_crown && !by_lattice {
        let modes = Modes {
            took: started.elapsed(),
            ..Modes::default()
        };
        return Ok((snapped, modes));
    }
    cancelled(cancel)?;
    let sparsely = if by_crown {
        sparse_with(rgb, p, w, h, cancel)?
    } else {
        Vec::new()
    };
    cancelled(cancel)?;
    let densely = if by_lattice {
        dense_with(rgb, p, w, h, cancel)?
    } else {
        Vec::new()
    };
    cancelled(cancel)?;
    // The modes' own mix where they have weight, the model's answer where neither has.
    let mixed: Vec<f32> = (0..w * h)
        .map(|i| {
            let (c, l) = (
                if by_crown { crowns[i] } else { 0. },
                if by_lattice { lattices[i] } else { 0. },
            );
            if c + l <= 0. {
                return smooth(p[i]);
            }
            let s = if by_crown { sparsely[i] } else { 0. };
            let d = if by_lattice { densely[i] } else { 0. };
            (c * s + l * d) / (c + l)
        })
        .collect();
    let refined = guided(luma, &mixed, w, h, 2, 1e-3);
    let out = (0..w * h)
        .map(|i| {
            let t = (crowns[i] + lattices[i]).min(1.);
            (1. - t) * snapped[i] + t * refined[i].clamp(0., 1.)
        })
        .collect();
    let modes = Modes {
        sparse: by_crown,
        dense: by_lattice,
        took: started.elapsed(),
    };
    Ok((out, modes))
}

/// The model's answer, `p`, snapped to the edges of the photo's `luma` by a guided
/// filter whose radius is a two-hundredth of the long side and settled by its curve: the
/// sky's edge where no mode is called for.
fn snapped(luma: &[f32], p: &[f32], width: usize, height: usize) -> Vec<f32> {
    guided(
        luma,
        p,
        width,
        height,
        (width.max(height) / 200).max(3),
        1e-3,
    )
    .iter()
    .map(|v| smooth(*v))
    .collect()
}

/// The sparse mode's and the dense mode's weights (0..=1) at `width` x `height` from
/// `kinds`: a crown's and a sure lattice's share of what the model sees, reached into the
/// sky around them, vetoed around water, eased and enlarged. They add up to at most one.
fn weights(kinds: &Kinds, width: usize, height: usize) -> (Vec<f32>, Vec<f32>) {
    let (gw, gh) = (kinds.width, kinds.height);
    let long = gw.max(gh) as f32;
    let reach = ((GATE * long).round() as usize).max(1);
    let ease = ((EASE * long).round() as usize).max(1);
    let cells = gw * gh;
    let seen: Vec<f32> = (0..cells)
        .map(|i| {
            kinds.built[i]
                + kinds.lattice[i]
                + kinds.vegetation[i]
                + kinds.terrain[i]
                + kinds.water[i]
                + kinds.unclear[i]
        })
        .collect();
    let owned = |i: usize| seen[i] >= OWNED;
    let share = |kind: &[f32], i: usize| if owned(i) { kind[i] / seen[i] } else { 0. };
    let crown: Vec<f32> = (0..cells)
        .map(|i| step(CROWN.0, CROWN.1, share(&kinds.vegetation, i)))
        .collect();
    let lattice: Vec<f32> = (0..cells)
        .map(|i| step(SURE_LATTICE.0, SURE_LATTICE.1, share(&kinds.lattice, i)))
        .collect();
    let water: Vec<f32> = (0..cells).map(|i| share(&kinds.water, i)).collect();
    // A crown's or a lattice's weight reaches into the sky around it, not onto a cell
    // the model sees as something else; water's veto reaches onto anything.
    let into_sky = |weights: &[f32]| -> Vec<f32> {
        let widest = widest(weights, gw, gh, reach);
        (0..cells)
            .map(|i| if owned(i) { weights[i] } else { widest[i] })
            .collect()
    };
    // The veto reaches past where an eased crown's weight could, so none is left at
    // the water's edge, and is eased itself, so the crown's weight fades out over rows.
    let water: Vec<f32> = water.iter().map(|v| step(VETO.0, VETO.1, *v)).collect();
    let veto = mean(&widest(&water, gw, gh, reach + 2 * ease), gw, gh, ease);
    let eased = |weights: &[f32]| -> Vec<f32> {
        let reached = into_sky(weights);
        let eased = mean(&reached, gw, gh, ease);
        (0..cells).map(|i| eased[i] * (1. - veto[i])).collect()
    };
    let (crown, lattice) = (eased(&crown), eased(&lattice));
    let grid = Rect {
        x: 0,
        y: 0,
        width: gw,
        height: gh,
    };
    let field = |values: &[f32]| -> Vec<f32> {
        resample(values, gw, grid, width, height)
            .into_iter()
            .map(|v| v.clamp(0., 1.))
            .collect()
    };
    let (mut crown, mut lattice) = (field(&crown), field(&lattice));
    for i in 0..width * height {
        let total = crown[i] + lattice[i];
        if total > 1. {
            crown[i] /= total;
            lattice[i] /= total;
        }
    }
    (crown, lattice)
}

/// The largest of `values` (`w` x `h`) within `r` of each cell, in a square.
fn widest(values: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut rows = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r + 1).min(w));
            rows[y * w + x] = values[y * w + a..y * w + b]
                .iter()
                .fold(0f32, |m, v| m.max(*v));
        }
    }
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        let (a, b) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            out[y * w + x] = (a..b).fold(0f32, |m, yy| m.max(rows[yy * w + x]));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use rawmakase_cancel::Cancel;

    use super::{blend, snapped, weights};
    use crate::auto::luma;
    use crate::kinds::{Kind, Kinds};
    use crate::process::{Coverage, RgbImage};
    use crate::refine::guided;
    use crate::sky_edge::fixtures::{
        answer, building, crown, crown_of, inputs, photo, tree_twig, truss_strut,
    };
    use crate::sky_edge::sparse::sparse;
    use rawmakase_inference::InferenceError;

    /// The model's quarter grid for a 1024 x 683 photo.
    const GRID: (usize, usize) = (256, 171);

    fn painted(kind: impl Fn(f32, f32) -> Option<Kind>) -> Kinds {
        Kinds::painted(GRID.0, GRID.1, |x, y| kind(x * 1024., y * 683.))
    }

    /// [`blend`]'s coverage (0..=1) for `image` and the model's `probability`
    /// (`model_width` x `model_height`), and the model's answer snapped as main does.
    fn blended(
        kinds: &Kinds,
        image: &RgbImage,
        probability: &[f32],
        model_width: usize,
        model_height: usize,
    ) -> (Vec<f32>, Vec<f32>) {
        let (rgb, soft, w, h) = inputs(image, probability, model_width, model_height);
        let grey = luma(image, w, h);
        let (out, _) = blend(&rgb, &grey, &soft, w, h, kinds, Cancel::NEVER).unwrap();
        (out, snapped(&grey, &soft, w, h))
    }

    /// [`blended`] as `auto::sky` stores it.
    fn finished(
        kinds: &Kinds,
        image: &RgbImage,
        probability: &[f32],
        model_width: usize,
        model_height: usize,
    ) -> Coverage {
        let (width, height) = crate::process::output_size(image);
        let (out, _) = blended(kinds, image, probability, model_width, model_height);
        Coverage {
            width,
            height,
            data: out
                .iter()
                .map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
                .collect(),
        }
    }

    /// Where `out` differs from `main` by more than rounding.
    fn differs(out: &[f32], main: &[f32]) -> Option<usize> {
        out.iter().zip(main).position(|(a, b)| (a - b).abs() > 1e-6)
    }

    /// The white facade with dark windows of [`building`], behind a bare tree whose
    /// upper half stands against the sky: the facade is built, the tree vegetation.
    #[test]
    fn a_facade_behind_twigs_gets_no_sky_and_the_gaps_between_the_twigs_do() {
        let window = |x: usize, y: usize| (x - 150) % 40 >= 28 && (y - 300) % 50 >= 16;
        let (img, p) = building(
            |x, y| {
                if window(x, y) {
                    [40, 42, 48]
                } else {
                    [236, 228, 210]
                }
            },
            280.0..340.,
        );
        let kinds = painted(|x, y| {
            if (200.0..420.).contains(&x) && (200.0..300.).contains(&y) {
                Some(Kind::Vegetation)
            } else if y >= 300. && (150.0..870.).contains(&x) {
                Some(Kind::Built)
            } else if y >= 500. {
                Some(Kind::Terrain)
            } else {
                None
            }
        });
        let cov = finished(&kinds, &img, &p, 1024, 683);
        let px = |x: usize, y: usize| cov.data[y * cov.width + x];
        for y in (340..480).step_by(4) {
            for x in (155..865).step_by(5).filter(|x| !tree_twig(*x, y)) {
                assert!(px(x, y) < 26, "facade {x},{y}: {}", px(x, y));
            }
        }
        let gaps: Vec<(usize, usize)> = (200..270)
            .filter(|y| y % 12 == 6)
            .flat_map(|y| (200..420).filter(|x| x % 14 == 7).map(move |x| (x, y)))
            .collect();
        assert!(gaps.len() > 50);
        for (x, y) in gaps {
            assert!(px(x, y) > 204, "gap {x},{y}: {}", px(x, y));
        }
        assert!(px(512, 150) > 240);
    }

    /// Whether a pixel is one of the pale panels of a half-timbered church beside
    /// [`crown`]'s crown, its body under the spire: panels 9 pixels square between dark
    /// timbers 3 pixels wide, painted as pale a blue as the sky.
    fn panel(x: usize, y: usize) -> bool {
        (780..920).contains(&x)
            && (260..400).contains(&y)
            && (x - 780) % 12 >= 3
            && (y - 260) % 12 >= 3
    }

    /// [`crown`] with a church under its spire: the crown is vegetation, the church
    /// built.
    #[test]
    fn a_church_beside_a_crown_gets_no_added_sky_and_the_crowns_gaps_do() {
        let (mut img, mut p) = crown();
        for y in 260..400 {
            for x in 780..920 {
                let colour = if panel(x, y) {
                    [150, 182, 222]
                } else {
                    [30, 35, 25]
                };
                let i = y * 1024 + x;
                img.data[3 * i..3 * i + 3].copy_from_slice(&colour);
                p[i] = 0.01;
            }
        }
        let kinds = painted(|x, y| {
            if y >= 400. {
                Some(Kind::Terrain)
            } else if (300.0..700.).contains(&x) && y >= 150. {
                Some(Kind::Vegetation)
            } else if (770.0..930.).contains(&x) && y >= 90. {
                Some(Kind::Built)
            } else {
                None
            }
        });
        let (out, main) = blended(&kinds, &img, &p, 1024, 683);
        for y in 105..395 {
            for x in 795..905 {
                let i = y * 1024 + x;
                assert!(
                    out[i] <= main[i] + 0.02,
                    "church {x},{y}: {} over {}",
                    out[i],
                    main[i]
                );
            }
        }
        let cov = finished(&kinds, &img, &p, 1024, 683);
        let px = |x: usize, y: usize| cov.data[y * cov.width + x];
        for (x, y) in [(307, 157), (499, 265), (691, 385), (403, 301)] {
            assert!(px(x, y) > 204, "gap {x},{y}: {}", px(x, y));
        }
        for (x, y) in [(301, 200), (500, 151), (601, 289)] {
            assert!(px(x, y) < 51, "branch {x},{y}: {}", px(x, y));
        }
        for (x, y) in [(788, 268), (848, 316), (908, 388)] {
            assert!(panel(x, y));
            assert!(px(x, y) < 26, "panel {x},{y}: {}", px(x, y));
        }
        assert!(px(100, 50) > 240 && px(500, 600) < 10);
    }

    /// The truss of a bridge's arch ([`truss_strut`]) on its deck, with water as pale
    /// as the sky on either side that the model leaves out.
    #[test]
    fn a_truss_beside_water_gets_sky_in_its_gaps_and_the_water_stays_the_models() {
        let is_water = |x: usize, y: usize| !(150..874).contains(&x) && y >= 300;
        let img = photo(1024, 683, |x, y| {
            if is_water(x, y) {
                [222, 226, 230]
            } else if y >= 500 || truss_strut(x, y) || (200..800).contains(&x) && y >= 390 {
                [40, 44, 46]
            } else {
                [226 + (y / 100) as u8, 228, 230]
            }
        });
        let probability = answer(1024, 683, |fx, fy| {
            let (x, y) = (fx * 1024., fy * 683.);
            if is_water(x as usize, y as usize) {
                0.01
            } else if y >= 500. || (200.0..800.).contains(&x) && y >= 250. {
                0.0003
            } else {
                0.99
            }
        });
        let kinds = painted(|x, y| {
            if is_water(x as usize, y as usize) {
                Some(Kind::Water)
            } else if (200.0..800.).contains(&x) && (250.0..390.).contains(&y) {
                Some(Kind::Lattice)
            } else if (200.0..800.).contains(&x) && (390.0..500.).contains(&y) {
                Some(Kind::Built)
            } else if y >= 500. {
                Some(Kind::Terrain)
            } else {
                None
            }
        });
        let (out, main) = blended(&kinds, &img, &probability, 1024, 683);
        for y in (320..683).step_by(3) {
            for x in (0..130).chain(894..1024).step_by(3) {
                let i = y * 1024 + x;
                assert!(
                    (out[i] - main[i]).abs() < 1e-4,
                    "water {x},{y}: {} for {}",
                    out[i],
                    main[i]
                );
            }
        }
        let cov = finished(&kinds, &img, &probability, 1024, 683);
        let px = |x: usize, y: usize| cov.data[y * cov.width + x];
        let clear = |x: usize, y: usize| {
            (x - 4..x + 5).all(|tx| (y - 4..y + 5).all(|ty| !truss_strut(tx, ty)))
        };
        let gaps: Vec<(usize, usize)> = (258..382)
            .step_by(5)
            .flat_map(|y| (210..790).step_by(5).map(move |x| (x, y)))
            .filter(|(x, y)| clear(*x, *y))
            .collect();
        let found = gaps.iter().filter(|(x, y)| px(*x, *y) > 204).count();
        assert!(
            found * 10 >= gaps.len() * 9,
            "{found} of {} gaps found",
            gaps.len()
        );
        for (x, y) in [(300, 290), (500, 292), (700, 291), (400, 338)] {
            assert!(truss_strut(x, y));
            assert!(px(x, y) < 51, "strut {x},{y}: {}", px(x, y));
        }
        assert!(px(60, 500) < 10 && px(960, 400) < 10, "water");
        assert!(px(500, 600) < 10 && px(500, 100) > 240);
    }

    /// A white column standing right beside a dark conifer, solid but for small gaps of
    /// sky, the model calling both not sky: the column is built, the crown vegetation.
    #[test]
    fn a_white_column_beside_a_crown_gets_no_sky_and_the_crowns_gaps_do() {
        let in_crown = |x: usize, y: usize| (300..600).contains(&x) && (150..400).contains(&y);
        let in_column = |x: usize, y: usize| (600..624).contains(&x) && (120..400).contains(&y);
        let img = photo(1024, 683, |x, y| {
            if y >= 400 {
                [40, 60, 30]
            } else if in_column(x, y) {
                [240, 238, 232]
            } else if in_crown(x, y) && ((x - 300) % 24 < 15 || (y - 150) % 24 < 15) {
                [30, 35, 25]
            } else {
                [110 + (y / 8) as u8, 150 + (y / 10) as u8, 225]
            }
        });
        let probability = answer(1024, 683, |fx, fy| {
            let (x, y) = ((fx * 1024.) as usize, (fy * 683.) as usize);
            if y >= 400 || in_crown(x, y) || in_column(x, y) {
                0.01
            } else {
                0.99
            }
        });
        let kinds = painted(|x, y| {
            let (x, y) = (x as usize, y as usize);
            if y >= 400 {
                Some(Kind::Terrain)
            } else if in_column(x, y) {
                Some(Kind::Built)
            } else if in_crown(x, y) {
                Some(Kind::Vegetation)
            } else {
                None
            }
        });
        let cov = finished(&kinds, &img, &probability, 1024, 683);
        let px = |x: usize, y: usize| cov.data[y * cov.width + x];
        for y in (125..395).step_by(3) {
            for x in 604..620 {
                assert!(px(x, y) < 26, "column {x},{y}: {}", px(x, y));
            }
        }
        for (x, y) in [(319, 169), (439, 289), (583, 385), (583, 169)] {
            assert!(px(x, y) > 204, "gap {x},{y}: {}", px(x, y));
        }
    }

    /// A pale gold finial with a dark bird perched on it, against a grey sky above a
    /// sunlit orange facade. The model doubts both (0.2) and takes them for a pole, a
    /// lattice; the facade is built. Neither colour lies between the sky's and the
    /// facade's: they are no sky the colours could find.
    #[test]
    fn a_pale_finial_and_a_dark_bird_against_a_grey_sky_get_no_sky() {
        let finial = |x: usize, y: usize| {
            let (dx, dy) = (x as f32 - 503., y as f32 - 170.);
            (500..506).contains(&x) && (150..400).contains(&y)
                || (dx / 6.).powi(2) + (dy / 10.).powi(2) <= 1.
        };
        let bird = |x: usize, y: usize| {
            let (dx, dy) = (x as f32 - 528., y as f32 - 214.);
            (dx / 9.).powi(2) + (dy / 5.).powi(2) <= 1.
        };
        let img = photo(1024, 683, |x, y| {
            if y >= 400 {
                [220, 142, 70]
            } else if bird(x, y) {
                [24, 25, 28]
            } else if finial(x, y) {
                [220, 209, 153]
            } else {
                [143, 141, 143]
            }
        });
        let probability = answer(1024, 683, |fx, fy| {
            let (x, y) = (fx * 1024., fy * 683.);
            if y >= 400. {
                return 0.0003;
            }
            let stem = (x - 503.).abs().max(150. - y).max(0.);
            let head = (x - 503.).hypot(y - 170.) - 8.;
            let perch = (x - 528.).hypot(y - 214.) - 7.;
            let d = stem.min(head).min(perch).max(0.);
            0.99 - 0.79 * (-d * d / 288.).exp()
        });
        let kinds = painted(|x, y| {
            if y >= 400. {
                Some(Kind::Built)
            } else if (480.0..556.).contains(&x) && y >= 136. {
                Some(Kind::Lattice)
            } else {
                None
            }
        });
        let cov = finished(&kinds, &img, &probability, 1024, 683);
        let px = |x: usize, y: usize| cov.data[y * cov.width + x];
        for (x, y) in [(503, 170), (503, 260), (503, 380), (499, 166)] {
            assert!(finial(x, y));
            assert!(px(x, y) < 51, "finial {x},{y}: {}", px(x, y));
        }
        for (x, y) in [(528, 214), (522, 213), (534, 215)] {
            assert!(bird(x, y));
            assert!(px(x, y) < 51, "bird {x},{y}: {}", px(x, y));
        }
        for (x, y) in [(470, 250), (580, 300), (540, 350), (300, 200)] {
            assert!(px(x, y) > 204, "sky {x},{y}: {}", px(x, y));
        }
    }

    /// A white plinth on dark grass behind a mesh fence, under a blue sky, the model
    /// sure neither is sky: the fence makes it a lattice. White lies beyond the sky's
    /// colour from the grass's, off the line between them: no sky either.
    #[test]
    fn a_white_plinth_behind_a_fence_gets_no_sky() {
        let mesh = |x: usize, y: usize| x % 14 < 2 || y % 14 < 2;
        let plinth = |x: usize, y: usize| (400..470).contains(&x) && (330..400).contains(&y);
        let img = photo(1024, 683, |x, y| {
            if y < 300 {
                [110 + (y / 8) as u8, 150 + (y / 10) as u8, 225]
            } else if (300..450).contains(&y) && mesh(x, y) {
                [30, 90, 60]
            } else if plinth(x, y) {
                [240, 238, 232]
            } else {
                [40, 60, 30]
            }
        });
        let probability = answer(
            1024,
            683,
            |_, fy| if fy * 683. < 300. { 0.99 } else { 0.0003 },
        );
        let kinds = painted(|_, y| match y {
            y if y < 300. => None,
            y if y < 450. => Some(Kind::Lattice),
            _ => Some(Kind::Terrain),
        });
        let cov = finished(&kinds, &img, &probability, 1024, 683);
        let px = |x: usize, y: usize| cov.data[y * cov.width + x];
        for y in (333..397).step_by(3) {
            for x in (403..467).step_by(3).filter(|x| !mesh(*x, y)) {
                assert!(px(x, y) < 26, "plinth {x},{y}: {}", px(x, y));
            }
        }
        assert!(px(500, 150) > 240 && px(500, 600) < 10);
    }

    /// A bare tree's few twigs, 2 pixels wide and far apart, across a large sky the
    /// model doubts (0.4) behind them: vegetation. The gaps are the sparse mode's,
    /// though the dense mode's size gate would leave the large sky out.
    #[test]
    fn far_apart_twigs_over_a_large_sky_get_the_sparse_modes_gaps() {
        let in_crown = |x: usize, y: usize| (100..924).contains(&x) && (40..400).contains(&y);
        let twig = |x: usize, y: usize| in_crown(x, y) && (x % 150 < 2 || y % 130 < 2);
        let img = photo(1024, 683, |x, y| {
            if y >= 400 {
                [40, 60, 30]
            } else if twig(x, y) {
                [35, 32, 30]
            } else {
                [120 + (y / 8) as u8, 155 + (y / 10) as u8, 222]
            }
        });
        let probability = answer(1024, 683, |fx, fy| {
            let (x, y) = ((fx * 1024.) as usize, (fy * 683.) as usize);
            if y >= 400 {
                0.0003
            } else if in_crown(x, y) {
                0.4
            } else {
                0.99
            }
        });
        let kinds = painted(|x, y| {
            if y >= 400. {
                Some(Kind::Terrain)
            } else if in_crown(x as usize, y as usize) {
                Some(Kind::Vegetation)
            } else {
                None
            }
        });
        let (rgb, soft, w, h) = inputs(&img, &probability, 1024, 683);
        let (out, _) = blended(&kinds, &img, &probability, 1024, 683);
        let sparsely = guided(&luma(&img, w, h), &sparse(&rgb, &soft, w, h), w, h, 2, 1e-3);
        let clear =
            |x: usize, y: usize| (x - 6..x + 7).all(|tx| (y - 6..y + 7).all(|ty| !twig(tx, ty)));
        let gaps: Vec<(usize, usize)> = (60..380)
            .step_by(9)
            .flat_map(|y| (120..904).step_by(9).map(move |x| (x, y)))
            .filter(|(x, y)| clear(*x, *y))
            .collect();
        assert!(gaps.len() > 2000);
        for &(x, y) in &gaps {
            let i = y * w + x;
            assert!(
                (out[i] - sparsely[i].clamp(0., 1.)).abs() < 1e-4,
                "gap {x},{y}: {} for sparse {}",
                out[i],
                sparsely[i]
            );
        }
        let cov = finished(&kinds, &img, &probability, 1024, 683);
        let px = |x: usize, y: usize| cov.data[y * cov.width + x];
        for &(x, y) in &gaps {
            assert!(px(x, y) > 204, "gap {x},{y}: {}", px(x, y));
        }
    }

    /// Budding twigs across the sky that the model, doubting (0.25), mostly takes for a
    /// waterfall: half water, a quarter vegetation. Water vetoes the modes, whatever the
    /// water turns out to be: the edge is the model's answer, as main's.
    #[test]
    fn sky_between_twigs_the_model_doubts_as_water_gets_the_models_answer() {
        let in_crown = |x: usize, y: usize| (100..924).contains(&x) && (40..400).contains(&y);
        let twig = |x: usize, y: usize| in_crown(x, y) && (x % 48 < 2 || y % 40 < 2);
        let img = photo(1024, 683, |x, y| {
            if y >= 400 {
                [40, 60, 30]
            } else if twig(x, y) {
                [35, 32, 30]
            } else {
                [120 + (y / 8) as u8, 155 + (y / 10) as u8, 222]
            }
        });
        let probability = answer(1024, 683, |fx, fy| {
            let (x, y) = ((fx * 1024.) as usize, (fy * 683.) as usize);
            if y >= 400 {
                0.0003
            } else if in_crown(x, y) {
                0.25
            } else {
                0.99
            }
        });
        let painted = painted(|_, y| (y >= 400.).then_some(Kind::Terrain));
        let share = |water: bool| -> Vec<f32> {
            (0..GRID.0 * GRID.1)
                .map(|i| {
                    let x = ((i % GRID.0) as f32 + 0.5) / GRID.0 as f32 * 1024.;
                    let y = ((i / GRID.0) as f32 + 0.5) / GRID.1 as f32 * 683.;
                    match (in_crown(x as usize, y as usize), water) {
                        (true, true) => 0.5,
                        (true, false) => 0.25,
                        (false, _) => 0.,
                    }
                })
                .collect()
        };
        let kinds = Kinds {
            water: share(true),
            vegetation: share(false),
            ..painted
        };
        let (out, main) = blended(&kinds, &img, &probability, 1024, 683);
        assert_eq!(differs(&out, &main), None);
    }

    /// A small grey cloud, 30 x 20 pixels at 1024 (smaller than the size gate's
    /// opening), in an open blue sky the model is sure of (0.99) and names no kind of,
    /// above terrain from row 500: the cloud stays sky, as the model says, at 1024 and
    /// at 2560 pixels, dark grey or medium grey.
    #[test]
    fn a_small_grey_cloud_in_sure_open_sky_stays_sky() {
        for (w, h) in [(1024, 683), (2560, 1707)] {
            let s = w as f32 / 1024.;
            let cloud = |x: usize, y: usize| {
                let (x, y) = (x as f32 / s, y as f32 / s);
                (500.0..530.).contains(&x) && (150.0..170.).contains(&y)
            };
            for grey in [[85, 90, 100], [110, 115, 125]] {
                let img = photo(w, h, |x, y| {
                    if y as f32 >= 500. * s {
                        [40, 60, 30]
                    } else if cloud(x, y) {
                        grey
                    } else {
                        [120, 160, 230]
                    }
                });
                let p = answer(w, h, |_, fy| if fy * 683. >= 500. { 0.0003 } else { 0.99 });
                let grid = (w / 4, h / 4);
                let kinds = Kinds::painted(grid.0, grid.1, |_, y| {
                    (y * 683. >= 500.).then_some(Kind::Terrain)
                });
                let cov = finished(&kinds, &img, &p, w, h);
                let px = |x: f32, y: f32| cov.data[(y * s) as usize * cov.width + (x * s) as usize];
                for (x, y) in [(515., 160.), (502., 152.), (528., 168.)] {
                    assert!(
                        px(x, y) > 240,
                        "{w}px, {grey:?} cloud at {x},{y}: {}",
                        px(x, y)
                    );
                }
                assert!(px(515., 600.) < 10);
            }
        }
    }

    /// The small grey cloud of the test above and a dark wire, 3 pixels wide, in an
    /// open blue sky the model is sure of (0.99), where it gives a faint share (0.001)
    /// of what it does not see as sky to vegetation or to water, as it does beside a
    /// tree: so faint a share is no kind, and the coverage is the model's answer, as
    /// main's, at 1024 and at 2560 pixels.
    #[test]
    fn a_faint_kind_in_sure_open_sky_leaves_the_models_answer() {
        for (w, h) in [(1024, 683), (2560, 1707)] {
            let s = w as f32 / 1024.;
            let cloud = |x: usize, y: usize| {
                let (x, y) = (x as f32 / s, y as f32 / s);
                (500.0..530.).contains(&x) && (150.0..170.).contains(&y)
            };
            let wire = |x: usize, y: usize| {
                let (x, y) = (x as f32 / s, y as f32 / s);
                (y - (300. + 0.1 * x)).abs() < 1.5 / s
            };
            let img = photo(w, h, |x, y| {
                if y as f32 >= 500. * s {
                    [40, 60, 30]
                } else if cloud(x, y) {
                    [85, 90, 100]
                } else if wire(x, y) {
                    [20, 22, 26]
                } else {
                    [120, 160, 230]
                }
            });
            let p = answer(w, h, |_, fy| if fy * 683. >= 500. { 0.0003 } else { 0.99 });
            for faint in [Kind::Vegetation, Kind::Water] {
                let mut kinds = Kinds::painted(w / 4, h / 4, |_, y| {
                    Some(if y * 683. >= 500. {
                        Kind::Terrain
                    } else {
                        faint
                    })
                });
                let share = match faint {
                    Kind::Vegetation => &mut kinds.vegetation,
                    _ => &mut kinds.water,
                };
                for v in share.iter_mut() {
                    *v *= 0.001;
                }
                let (out, main) = blended(&kinds, &img, &p, w, h);
                assert_eq!(differs(&out, &main), None, "{w}px, {faint:?}");
            }
        }
    }

    /// Reeds standing in a lake whose water between them is as pale as the sky above,
    /// the model doubting the water (0.45) and seeing the reeds as half vegetation, half
    /// water: water's veto leaves it all the model's answer, as main's.
    #[test]
    fn water_between_reeds_stays_out_of_the_sky() {
        let reed = |x: usize, y: usize| (200..800).contains(&x) && y >= 330 && x % 20 < 3;
        let in_reeds = |x: f32, y: f32| (200.0..800.).contains(&x) && y >= 330.;
        let img = photo(1024, 683, |x, y| {
            if y < 300 {
                [170, 190, 225]
            } else if reed(x, y) {
                [45, 50, 30]
            } else {
                [168, 188, 222]
            }
        });
        let probability = answer(
            1024,
            683,
            |_, fy| if fy * 683. < 300. { 0.99 } else { 0.45 },
        );
        let mut kinds = painted(|_, y| (y >= 300.).then_some(Kind::Water));
        for i in 0..GRID.0 * GRID.1 {
            let x = ((i % GRID.0) as f32 + 0.5) / GRID.0 as f32 * 1024.;
            let y = ((i / GRID.0) as f32 + 0.5) / GRID.1 as f32 * 683.;
            if in_reeds(x, y) {
                (kinds.vegetation[i], kinds.water[i]) = (0.35, 0.35);
            }
        }
        let water: Vec<(usize, usize)> = (340..680)
            .step_by(7)
            .flat_map(|y| (210..790).step_by(7).map(move |x| (x, y)))
            .filter(|(x, y)| (x - 4..x + 5).all(|tx| !reed(tx, *y)))
            .collect();
        assert!(water.len() > 1000);
        let (out, main) = blended(&kinds, &img, &probability, 1024, 683);
        assert_eq!(differs(&out, &main), None);
    }

    /// A tree crown standing on a shore, the water below it: the crown's weight is all
    /// the sparse mode's above the water and none of it at the water, easing from one to
    /// the other over a dozen rows or more instead of stepping.
    #[test]
    fn a_crown_on_a_shore_eases_from_the_sparse_mode_to_the_models_answer() {
        let kinds = painted(|x, y| {
            if y >= 420. {
                Some(Kind::Water)
            } else if (300.0..700.).contains(&x) && y >= 150. {
                Some(Kind::Vegetation)
            } else {
                None
            }
        });
        let (crowns, lattices) = weights(&kinds, 1024, 683);
        assert!(lattices.iter().all(|v| *v == 0.));
        let at = |x: usize, y: usize| crowns[y * 1024 + x];
        assert!(at(500, 250) > 0.99, "crown {}", at(500, 250));
        assert!(at(500, 160) > 0.99 && at(500, 145) > 0.9, "outer twigs");
        assert!(at(500, 480) < 0.01, "water {}", at(500, 480));
        assert!(at(100, 100) < 0.01, "open sky {}", at(100, 100));
        for x in [320, 500, 680] {
            for y in 100..682 {
                let step = (at(x, y + 1) - at(x, y)).abs();
                assert!(step < 0.08, "{x},{y}: steps by {step}");
            }
        }
    }

    /// A lattice the model is unsure of (a third of what it sees there) gets the
    /// model's answer; one it is sure of gets the dense mode.
    #[test]
    fn only_a_lattice_the_model_is_sure_of_gets_the_dense_mode() {
        let in_truss = |x: f32, y: f32| (200.0..800.).contains(&x) && (250.0..390.).contains(&y);
        for (lattice, built, expected) in [(1. / 3., 2. / 3., 0.), (0.9, 0.1, 1.)] {
            let mut kinds = painted(|x, y| in_truss(x, y).then_some(Kind::Lattice));
            for i in 0..GRID.0 * GRID.1 {
                if kinds.lattice[i] > 0. {
                    (kinds.lattice[i], kinds.built[i]) = (lattice, built);
                }
            }
            let (_, lattices) = weights(&kinds, 1024, 683);
            let got = lattices[320 * 1024 + 500];
            assert!((got - expected).abs() < 0.01, "{lattice}: {got}");
        }
    }

    /// Without any kind, or with only kinds no mode is for, the coverage is the model's
    /// answer snapped as main snaps it.
    #[test]
    fn without_a_crown_or_a_lattice_the_coverage_is_mains() {
        let (img, p) = crown();
        for kinds in [
            painted(|_, _| None),
            painted(|_, y| (y >= 150.).then_some(Kind::Built)),
            painted(|_, y| (y >= 150.).then_some(Kind::Terrain)),
            painted(|_, y| (y >= 150.).then_some(Kind::Water)),
            painted(|_, y| (y >= 150.).then_some(Kind::Unclear)),
        ] {
            let (out, main) = blended(&kinds, &img, &p, 1024, 683);
            assert_eq!(differs(&out, &main), None);
        }
    }

    /// The church of `a_church_beside_a_crown_gets_no_added_sky_and_the_crowns_gaps_do`
    /// beside [`crown`]'s crown, along row 330: from the crown's edge (x 700) across the
    /// open sky where the kinds change, from vegetation to none at x 700 and to built at
    /// x 770, and the crown's weight fades out, into the church's panels (x 780 to 920).
    /// Pixels within 8 of the model's own edges, at x 700 (the crown's side) and x 780 and
    /// 920 (the church's), are skipped: there the coverage rightly steps from foreground
    /// to sky, as main's does. Elsewhere no neighbouring pixels differ by more than 0.08:
    /// no seam where one mode gives way to the model's answer.
    #[test]
    fn a_crown_beside_a_church_shows_no_seam_in_the_coverage() {
        let (mut img, mut p) = crown();
        for y in 260..400 {
            for x in 780..920 {
                let colour = if panel(x, y) {
                    [150, 182, 222]
                } else {
                    [30, 35, 25]
                };
                let i = y * 1024 + x;
                img.data[3 * i..3 * i + 3].copy_from_slice(&colour);
                p[i] = 0.01;
            }
        }
        let kinds = painted(|x, y| {
            if y >= 400. {
                Some(Kind::Terrain)
            } else if (300.0..700.).contains(&x) && y >= 150. {
                Some(Kind::Vegetation)
            } else if (770.0..930.).contains(&x) && y >= 90. {
                Some(Kind::Built)
            } else {
                None
            }
        });
        let cov = finished(&kinds, &img, &p, 1024, 683);
        let y = 330;
        let px = |x: usize| f32::from(cov.data[y * cov.width + x]);
        let near_the_models_edge =
            |x: usize| [700, 780, 920].iter().any(|edge| x.abs_diff(*edge) <= 8);
        let walked: Vec<usize> = (700..920)
            .filter(|x| !near_the_models_edge(*x) && !near_the_models_edge(x + 1))
            .collect();
        assert!(walked.len() > 150);
        for x in walked {
            let step = (px(x + 1) - px(x)).abs();
            assert!(step <= 0.08 * 255., "{x},{y}: {} to {}", px(x), px(x + 1));
        }
    }

    /// [`crown`]'s crown at 2560 pixels wide (twice the app's input), the crown
    /// vegetation: the sky between its branches is found and the branches are not, as at
    /// 1024.
    #[test]
    fn sky_between_branches_is_found_at_the_skys_input_size() {
        let (img, p) = crown_of(
            2560,
            1707,
            |y| [110 + (y / 8) as u8, 150 + (y / 10) as u8, 225],
            [30, 35, 25],
        );
        let s = 2560. / 1024.;
        let kinds = Kinds::painted(640, 426, |x, y| {
            let (x, y) = (x * 1024., y * 683.);
            if y >= 400. {
                Some(Kind::Terrain)
            } else if (300.0..700.).contains(&x) && y >= 150. {
                Some(Kind::Vegetation)
            } else {
                None
            }
        });
        let cov = finished(&kinds, &img, &p, 2560, 1707);
        assert_eq!((cov.width, cov.height), (2560, 1707));
        let px = |x: f32, y: f32| cov.data[(y * s) as usize * cov.width + (x * s) as usize];
        for (x, y) in [
            (307.5, 157.5),
            (499.5, 265.5),
            (691.5, 385.5),
            (403.5, 301.5),
        ] {
            assert!(px(x, y) > 204, "gap {x},{y}: {}", px(x, y));
        }
        for (x, y) in [(301.5, 200.), (500., 151.5), (601.5, 289.)] {
            assert!(px(x, y) < 51, "branch {x},{y}: {}", px(x, y));
        }
        assert!(px(100., 50.) > 240 && px(500., 600.) < 10);
    }

    #[test]
    fn a_raised_cancel_flag_cancels() {
        let (img, p) = crown();
        let (rgb, soft, w, h) = inputs(&img, &p, 1024, 683);
        let grey = luma(&img, w, h);
        let kinds = painted(|_, y| (y >= 150.).then_some(Kind::Vegetation));
        let result = blend(
            &rgb,
            &grey,
            &soft,
            w,
            h,
            &kinds,
            Cancel::new(&AtomicBool::new(true)),
        );
        assert!(matches!(result, Err(InferenceError::Cancelled)));
    }

    #[test]
    fn a_raised_cancel_flag_cancels_before_the_edge_is_snapped() {
        let (img, p) = crown();
        let (rgb, soft, w, h) = inputs(&img, &p, 1024, 683);
        let grey = luma(&img, w, h);
        let kinds = painted(|_, _| Some(Kind::Terrain));
        let result = blend(
            &rgb,
            &grey,
            &soft,
            w,
            h,
            &kinds,
            Cancel::new(&AtomicBool::new(true)),
        );
        assert!(matches!(result, Err(InferenceError::Cancelled)));
    }
}
