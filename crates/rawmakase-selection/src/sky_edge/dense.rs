//! The dense mode: the sky in the small gaps of a truss or a lattice (and, on its own, of
//! a crown).
//!
//! Near the model's boundary each pixel is placed on the line between the local sky
//! colour and the local foreground colour, and so it stands wherever the colours
//! disagree with the model over a large area (a wall, a roof, water): only small
//! departures, the size of a gap or a spire, are taken. A colour far off that line is
//! neither (a white column under a blue sky, a gilt finial or a dark bird against a
//! grey one over a sunlit wall): it may say there is less sky than the model sees,
//! never more.

use rawmakase_cancel::Cancel;

use super::{
    BAND, GATE, NEAR, SEPARATION, SKY_LIKE, SURE_GROUND, SURE_SKY, UNIFORM, along, cancelled,
    dilate, open, pull_push, questioned_seed, reached, settle, spread, step,
};
use crate::refine::smooth;
use rawmakase_inference::InferenceError;

/// Distance of a colour from the line between the sky's and the ground's (as a share of
/// that line's length) over which it goes from lying on the line, a mix of the two, to
/// lying clearly off it, neither.
const OFF_LINE: (f32, f32) = (0.15, 0.3);

/// [`dense_with`], never cancelled.
#[cfg(test)]
pub(crate) fn dense(rgb: &[[f32; 3]], p: &[f32], width: usize, height: usize) -> Vec<f32> {
    dense_with(rgb, p, width, height, Cancel::NEVER).expect("a flag that is never raised")
}

/// Sky coverage (0..=1) from the model's answer, `p` (0..=1 at `width` x `height`),
/// refined with `rgb`, the photo's colours (sRGB / 255) at the same size. `cancel` is
/// checked between the passes.
pub(crate) fn dense_with(
    rgb: &[[f32; 3]],
    p: &[f32],
    width: usize,
    height: usize,
    cancel: Cancel<'_>,
) -> Result<Vec<f32>, InferenceError> {
    let (w, h) = (width, height);
    let long = w.max(h) as f32;
    let gate = ((GATE * long).round() as usize).max(2);
    let band = ((BAND * long).round() as usize).max(gate);
    let spread = spread(rgb, w, h, gate);
    let sky_sure: Vec<bool> = p
        .iter()
        .zip(&spread)
        .map(|(p, s)| *p > SURE_SKY && *s < UNIFORM)
        .collect();
    let questioned = dilate(&questioned_seed(p), w, h, band);
    let ground_sure: Vec<bool> = questioned.iter().map(|q| !q).collect();
    if !sky_sure.contains(&true) || !ground_sure.contains(&true) {
        return Ok(p.iter().map(|p| smooth(*p)).collect());
    }
    let ground = pull_push(rgb, &ground_sure, w, h);
    // The sky's colour from where it is smooth first; then also from wherever the model
    // sees sky (sure, or fading) and the colour clearly sides with that sky: streaks of
    // cloud or a duller patch are measured against their own sky, not a brighter one
    // farther off; a spire, a twig or a colour off the line from the ground to that sky
    // (a dark bird before a grey sky over a sunlit wall) is not sky-like enough to count.
    let smooth_sky = pull_push(rgb, &sky_sure, w, h);
    let sky_like: Vec<bool> = (0..w * h)
        .map(|i| {
            sky_sure[i]
                || (p[i] >= SURE_GROUND
                    && along(rgb[i], smooth_sky[i], ground[i])
                        * on_line(rgb[i], smooth_sky[i], ground[i])
                        >= SKY_LIKE)
        })
        .collect();
    let sky = pull_push(rgb, &sky_like, w, h);
    cancelled(cancel)?;
    let unknown: Vec<bool> = (0..w * h)
        .map(|i| !sky_sure[i] && !ground_sure[i])
        .collect();
    // What the colours say (the line between the local sky and ground colours), and
    // how far they can be trusted to say it.
    let (colour, trust): (Vec<f32>, Vec<f32>) = (0..w * h)
        .map(|i| {
            if !unknown[i] {
                return (p[i], 0.);
            }
            let (b, f) = (sky[i], ground[i]);
            let length2 = (0..3).map(|k| (b[k] - f[k]).powi(2)).sum::<f32>();
            if length2 <= 0. {
                return (p[i], 0.);
            }
            // Where the model sees foreground, the colours must also vary there: a crown
            // as dark as the night behind it, or water or a wall coloured like the sky,
            // does not tell the two apart, and the model's answer stands. On the model's
            // own fading edge it is enough that sky and ground differ.
            let fading = (SURE_GROUND..=SURE_SKY).contains(&p[i]);
            let apart = if fading {
                length2.sqrt()
            } else {
                length2.sqrt().min(2. * spread[i])
            };
            // A colour off the line is not the sky's: it may say there is less sky than
            // the model sees, never more.
            let along = along(rgb[i], b, f);
            let said = along.min(p[i] + on_line(rgb[i], b, f) * (along - p[i]));
            (said, step(SEPARATION.0, SEPARATION.1, apart))
        })
        .unzip();
    let mixed: Vec<f32> = (0..w * h)
        .map(|i| p[i] + trust[i] * (colour[i] - p[i]))
        .collect();
    // Where the colours disagree with the model, judged on the colours alone: an even
    // area the colours cannot be trusted over is still one area, not its rims.
    let add: Vec<bool> = (0..w * h)
        .map(|i| unknown[i] && colour[i] > 0.5 && p[i] <= 0.5)
        .collect();
    let remove: Vec<bool> = (0..w * h)
        .map(|i| unknown[i] && colour[i] < 0.5 && p[i] > 0.5)
        .collect();
    let (large_add, large_remove) = (open(&add, w, h, gate), open(&remove, w, h, gate));
    cancelled(cancel)?;
    let small: Vec<bool> = (0..w * h)
        .map(|i| (add[i] && !large_add[i]) || (remove[i] && !large_remove[i]))
        .collect();
    // A departure itself, and the few pixels of its soft edge, take the colours' answer.
    // Around a gap the model put inside the foreground (in a crown, not along an
    // outline), partly-sky twigs take their share too, but that never lowers sky the
    // model found. Nothing near a large departure does, the model's unsure edge there
    // included.
    let large: Vec<bool> = (0..w * h)
        .map(|i| large_add[i] || large_remove[i])
        .collect();
    let small_add: Vec<bool> = (0..w * h).map(|i| add[i] && !large_add[i]).collect();
    let beside_sky = dilate(&p.iter().map(|p| *p > 0.5).collect::<Vec<_>>(), w, h, 1);
    let outline = reached(&small_add, &beside_sky, w, h);
    let gaps: Vec<bool> = (0..w * h).map(|i| small_add[i] && !outline[i]).collect();
    let (edge, around) = (dilate(&small, w, h, NEAR), dilate(&gaps, w, h, gate));
    let beside_large = dilate(&large, w, h, gate);
    // The model's unsure edge is the colours' to decide only where it is the edge of the
    // sky: a faint wisp it draws on water or a wall, cut off from the sky, is not.
    let edge_of_sky = reached(&questioned_seed(p), &sky_sure, w, h);
    let taken: Vec<bool> = (0..w * h)
        .map(|i| edge[i] || (around[i] && p[i] <= 0.5))
        .collect();
    Ok((0..w * h)
        .map(|i| {
            let unsure = (SURE_GROUND..=SURE_SKY).contains(&p[i]);
            if unknown[i] && !beside_large[i] && ((unsure && edge_of_sky[i]) || taken[i]) {
                settle(mixed[i])
            } else {
                smooth(p[i])
            }
        })
        .collect())
}

/// How near (1) or far off (0) the line from `ground` to `sky` `colour` lies, by its
/// distance from the stretch between them ramped over [`OFF_LINE`]: 0 when the two are
/// the same colour.
fn on_line(colour: [f32; 3], sky: [f32; 3], ground: [f32; 3]) -> f32 {
    let length2 = (0..3).map(|k| (sky[k] - ground[k]).powi(2)).sum::<f32>();
    if length2 <= 0. {
        return 0.;
    }
    let t = along(colour, sky, ground);
    let off2 = (0..3)
        .map(|k| (colour[k] - ground[k] - t * (sky[k] - ground[k])).powi(2))
        .sum::<f32>();
    1. - step(OFF_LINE.0, OFF_LINE.1, (off2 / length2).sqrt())
}

#[cfg(test)]
mod tests {
    use super::dense;
    use crate::sky_edge::fixtures::{self, answer, photo, run, truss_strut};

    #[test]
    fn sky_is_where_the_model_says_with_its_edge_on_the_photos() {
        fixtures::sky_is_where_the_model_says_with_its_edge_on_the_photos(dense);
    }

    #[test]
    fn water_under_the_sky_is_left_out_when_the_model_leaves_it_out() {
        fixtures::water_under_the_sky_is_left_out_when_the_model_leaves_it_out(dense);
    }

    #[test]
    fn sky_below_leaves_along_the_top_is_kept() {
        fixtures::sky_below_leaves_along_the_top_is_kept(dense);
    }

    #[test]
    fn sky_between_branches_is_found_and_the_branches_are_not() {
        fixtures::sky_between_branches_is_found_and_the_branches_are_not(dense);
    }

    #[test]
    fn a_sky_coloured_wall_next_to_the_sky_stays_out_up_to_its_edge() {
        fixtures::a_sky_coloured_wall_next_to_the_sky_stays_out_up_to_its_edge(dense);
    }

    #[test]
    fn twigs_in_a_crown_with_gaps_are_partly_sky() {
        fixtures::twigs_in_a_crown_with_gaps_are_partly_sky(dense);
    }

    #[test]
    fn a_reflection_broken_by_reflected_trees_stays_out() {
        fixtures::reflection_stays_out(dense, 0.12);
    }

    /// The model's own faint answer on water, away from the sky, is no more sky than
    /// it was before the colours were asked.
    #[test]
    fn a_reflection_the_model_is_a_little_unsure_of_stays_out() {
        fixtures::reflection_stays_out(dense, 0.3);
    }

    #[test]
    fn a_faint_wisp_on_the_water_away_from_the_sky_stays_out() {
        fixtures::a_faint_wisp_on_the_water_away_from_the_sky_stays_out(dense);
    }

    #[test]
    fn a_streaky_sky_where_the_model_fades_stays_entirely_selected() {
        fixtures::a_streaky_sky_where_the_model_fades_stays_entirely_selected(dense);
    }

    #[test]
    fn pale_wood_in_a_trunk_beside_the_sky_stays_out() {
        fixtures::pale_wood_in_a_trunk_beside_the_sky_stays_out(dense);
    }

    #[test]
    fn a_streaky_sky_beside_a_trunk_stays_entirely_selected() {
        fixtures::a_streaky_sky_beside_a_trunk_stays_entirely_selected(dense);
    }

    #[test]
    fn a_duller_patch_of_sky_where_the_model_fades_stays_entirely_selected() {
        fixtures::a_duller_patch_of_sky_where_the_model_fades_stays_entirely_selected(dense);
    }

    #[test]
    fn a_cloudy_sky_away_from_its_edge_is_entirely_selected() {
        fixtures::a_cloudy_sky_away_from_its_edge_is_entirely_selected(dense);
    }

    #[test]
    fn dark_sky_behind_dark_trees_keeps_the_models_answer() {
        fixtures::dark_sky_behind_dark_trees_keeps_the_models_answer(dense);
    }

    #[test]
    fn tiny_and_extreme_photos_do_not_break() {
        fixtures::tiny_and_extreme_photos_do_not_break(dense);
    }

    #[test]
    fn all_sky_is_the_models_answer() {
        fixtures::all_sky_is_the_models_answer(dense);
    }

    /// The truss of a bridge's arch ([`truss_strut`]), and a model answer that, like
    /// UperNet's, is all but certain the whole truss down to the deck is not sky: the
    /// sparse mode's truss test, run through the dense mode, which `blend` gives a
    /// lattice.
    #[test]
    fn sky_between_the_struts_of_a_truss_is_found_by_the_dense_mode() {
        let img = photo(1024, 683, |x, y| {
            if y >= 500 || truss_strut(x, y) || (200..800).contains(&x) && y >= 390 {
                [40, 44, 46]
            } else {
                [226 + (y / 100) as u8, 228, 230]
            }
        });
        let probability = answer(1024, 683, |fx, fy| {
            let (x, y) = (fx * 1024., fy * 683.);
            if y >= 500. || (200.0..800.).contains(&x) && y >= 250. {
                0.0003
            } else {
                0.99
            }
        });
        let cov = run(dense, &img, &probability, 1024, 683);
        let px = |x: usize, y: usize| cov.data[y * cov.width + x];
        let clear = |x: usize, y: usize| {
            (x - 4..x + 5).all(|tx| (y - 4..y + 5).all(|ty| !truss_strut(tx, ty)))
        };
        let gaps: Vec<(usize, usize)> = (258..382)
            .step_by(5)
            .flat_map(|y| (210..790).step_by(5).map(move |x| (x, y)))
            .filter(|(x, y)| clear(*x, *y))
            .collect();
        assert!(gaps.len() > 100);
        let missed: Vec<_> = gaps.iter().filter(|(x, y)| px(*x, *y) <= 204).collect();
        assert!(
            missed.is_empty(),
            "{} of {} gaps missed, e.g. {:?}: {}",
            missed.len(),
            gaps.len(),
            missed[0],
            px(missed[0].0, missed[0].1)
        );
        for (x, y) in [(300, 290), (500, 292), (700, 291), (400, 338)] {
            assert!(truss_strut(x, y));
            assert!(px(x, y) < 51, "strut {x},{y}: {}", px(x, y));
        }
        assert!(px(500, 450) < 26, "deck {}", px(500, 450));
        assert!(px(500, 600) < 10);
    }
}
