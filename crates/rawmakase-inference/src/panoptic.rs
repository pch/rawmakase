//! Reading the panoptic model's answer: which pixels are a person or an animal. The model
//! returns a fixed number of "queries", each a class guess with a mask; the queries it is
//! sure of compete for every pixel, as the model's authors decode it, and the winners'
//! masks make the instances. Pure: no runtime is involved.
use crate::auto::SIDE;
use crate::error::InferenceError;
use crate::manifest::PanopticSpec;
use crate::process::{Rect, resample};

/// One person or animal the model found: its mask on the [`SIDE`] grid (the pixels
/// where it won the competition, 0..=1).
pub struct Instance {
    pub class: usize,
    pub score: f32,
    pub mask: Vec<f32>,
}

/// What the model found: each person and animal, largest first.
pub struct Labels {
    pub instances: Vec<Instance>,
}

/// Fewest grid cells (of [`SIDE`]²) an instance must cover, so a few cells of noise
/// are not a subject while a distant person (about 6×6 cells) still is.
const MIN_CELLS: usize = 36;

/// Most instances kept: a crowd is not one photo's subject.
const MOST_INSTANCES: usize = 8;

/// Decodes `logits` (`queries` x `classes`, the last class meaning "nothing") and the
/// queries' mask logits (`queries` x `height` x `width`).
pub fn decode(
    spec: &PanopticSpec,
    logits: &[f32],
    masks: &[f32],
    queries: usize,
    classes: usize,
    width: usize,
    height: usize,
) -> Result<Labels, InferenceError> {
    let pixels = width * height;
    if classes < 3
        || queries == 0
        || pixels == 0
        || logits.len() != queries * classes
        || masks.len() != queries * pixels
        || logits.iter().chain(masks).any(|v| !v.is_finite())
    {
        return Err(InferenceError::OutputInvalid(format!(
            "the panoptic output has {} logits and {} mask values for {queries} queries, \
             {classes} classes and {width}x{height} masks",
            logits.len(),
            masks.len()
        )));
    }
    // The queries the model is sure of, with their class and confidence.
    let mut sure: Vec<(usize, usize, f32)> = Vec::new();
    for q in 0..queries {
        let row = &logits[q * classes..(q + 1) * classes];
        let max = row.iter().copied().fold(f32::MIN, f32::max);
        let sum: f32 = row.iter().map(|v| (v - max).exp()).sum();
        let (class, top) = row[..classes - 1]
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .expect("at least two classes");
        let confidence = (top - max).exp() / sum;
        if class != 0 && confidence > spec.confidence {
            sure.push((q, class, confidence));
        }
    }
    let mut maps: Vec<Vec<f32>> = vec![vec![0f32; pixels]; sure.len()];
    for p in 0..pixels {
        let mut best = (0.0f32, None::<(usize, f32)>);
        for (k, &(q, _, confidence)) in sure.iter().enumerate() {
            let prob = 1.0 / (1.0 + (-masks[q * pixels + p]).exp());
            let score = confidence * prob;
            if score > best.0 {
                best = (score, Some((k, prob)));
            }
        }
        if let Some((k, prob)) = best.1 {
            let class = sure[k].1;
            if spec.is_subject(class) {
                maps[k][p] = prob;
            }
        }
    }
    let whole = Rect {
        x: 0,
        y: 0,
        width,
        height,
    };
    let grid = |map: &[f32]| -> Vec<f32> {
        resample(map, width, whole, SIDE, SIDE)
            .into_iter()
            .map(|v| v.clamp(0.0, 1.0))
            .collect()
    };
    let mut instances: Vec<Instance> = sure
        .iter()
        .zip(&maps)
        .filter(|((_, class, _), _)| spec.is_subject(*class))
        .map(|(&(_, class, score), map)| Instance {
            class,
            score,
            mask: grid(map),
        })
        .filter(|i| i.mask.iter().filter(|v| **v > 0.5).count() >= MIN_CELLS)
        .collect();
    let area = |i: &Instance| i.mask.iter().filter(|v| **v > 0.5).count();
    instances.sort_by_key(|i| std::cmp::Reverse(area(i)));
    instances.truncate(MOST_INSTANCES);
    Ok(Labels { instances })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::PANOPTIC;

    const CLASSES: usize = 251;

    /// Two queries on a 8x4 mask: query 0 says `first` on the left half, query 1 says
    /// `second` on the right half; the rest say nothing.
    fn scene(first: (usize, f32), second: (usize, f32)) -> (Vec<f32>, Vec<f32>) {
        let (w, h, queries) = (8usize, 4usize, 3usize);
        let mut logits = vec![0f32; queries * CLASSES];
        // Query 2 is "nothing" (the last class).
        logits[2 * CLASSES + CLASSES - 1] = 20.0;
        for (q, (class, sure)) in [first, second].into_iter().enumerate() {
            logits[q * CLASSES + class] = sure;
        }
        let mut masks = vec![-9f32; queries * w * h];
        for p in 0..w * h {
            let left = p % w < w / 2;
            masks[p] = if left { 9.0 } else { -9.0 };
            masks[w * h + p] = if left { -9.0 } else { 9.0 };
        }
        (logits, masks)
    }
    fn at(map: &[f32], fx: f32, fy: f32) -> f32 {
        map[(fy * SIDE as f32) as usize * SIDE + (fx * SIDE as f32) as usize]
    }

    #[test]
    fn people_and_animals_are_instances_and_nothing_else_is() {
        let (logits, masks) = scene((1, 12.0), (187, 12.0));
        let labels = decode(&PANOPTIC, &logits, &masks, 3, CLASSES, 8, 4).unwrap();
        assert_eq!(labels.instances.len(), 1);
        let person = &labels.instances[0];
        assert_eq!(person.class, 1);
        assert!(at(&person.mask, 0.1, 0.5) > 0.95 && at(&person.mask, 0.9, 0.5) < 0.05);
        // An animal counts as a subject too, and a car is neither.
        let (logits, masks) = scene((18, 12.0), (3, 12.0));
        let labels = decode(&PANOPTIC, &logits, &masks, 3, CLASSES, 8, 4).unwrap();
        assert_eq!(labels.instances.len(), 1);
        assert_eq!(labels.instances[0].class, 18);
    }

    #[test]
    fn a_guess_the_model_is_unsure_of_does_not_count() {
        // Class 1 barely ahead of class 2: confidence well under the bar.
        let (mut logits, masks) = scene((1, 1.0), (187, 12.0));
        logits[2] = 0.9;
        let labels = decode(&PANOPTIC, &logits, &masks, 3, CLASSES, 8, 4).unwrap();
        assert!(labels.instances.is_empty());
    }

    #[test]
    fn mismatched_or_damaged_output_is_an_error() {
        let (logits, masks) = scene((1, 12.0), (187, 12.0));
        assert!(decode(&PANOPTIC, &logits[1..], &masks, 3, CLASSES, 8, 4).is_err());
        assert!(decode(&PANOPTIC, &logits, &masks[1..], 3, CLASSES, 8, 4).is_err());
        let mut bad = masks;
        bad[3] = f32::NAN;
        assert!(decode(&PANOPTIC, &logits, &bad, 3, CLASSES, 8, 4).is_err());
    }
}
