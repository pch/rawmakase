//! The preset a Lightroom edit looks made with when it records none.
//!
//! Lightroom records the last preset applied as `crs:Preset`, but most edits
//! lose the record (copied, pasted or synced settings carry none). A preset is
//! scored by the share of its own settings the edit still has, as a photo's
//! adaptation changes some of them: one that sets at least
//! [`MIN_SETTINGS`] must reach [`MIN_SHARE`], and a smaller one (a tone curve
//! alone, say) must fit whole, since a few settings match by chance. The best
//! must lead every preset of another name by [`MIN_LEAD`]. Of two that fit
//! whole, the larger wins only when it holds every setting of the smaller (a
//! curve inside a look made with it); otherwise both were applied, one on top
//! of the other, and neither is named. Checked against edits that do record
//! their preset, these thresholds named a wrong preset rarely, where looser
//! ones did often; precision matters more than how many are found.
//!
//! RAWmakase's own presets made no Lightroom edit and are left out, and a
//! straight tone curve counts as no setting: nearly every edit has one, so a
//! preset of straight curves would fit almost any.
//!
//! It is a guess and is shown as one ("Looks like …"); a recorded preset always
//! comes first. Only Camera Raw XMP edits are compared, not Lightroom Classic's
//! Lua settings, whose values are written differently.
use crate::{color::curve::ToneCurve, model::recipe::AppliedPreset, xmp::Preset};

/// Settings that say what the camera did or what a preset needs, not what it
/// sets.
const NOT_COMPARED: &[&str] = &[
    "AsShotTemperature",
    "AsShotTint",
    "RAWmakaseLookUUID",
    "RequiresRGBTables",
];

/// The share of a preset's settings an edit must have.
pub const MIN_SHARE: f32 = 0.6;
/// How far, in share, the best preset must lead any other of another name.
pub const MIN_LEAD: f32 = 0.15;
/// A preset of fewer settings counts only when the edit has all of them.
pub const MIN_SETTINGS: usize = 5;

/// What each preset sets, ready to be compared with many edits.
#[derive(Clone, Debug, Default)]
pub struct Fingerprints {
    presets: Vec<Fingerprint>,
}

#[derive(Clone, Debug)]
struct Fingerprint {
    name: String,
    uuid: String,
    settings: Vec<(String, String)>,
    curves: Vec<(String, ToneCurve)>,
    look: String,
}
impl Fingerprint {
    /// How many settings it sets: a match on more is more specific.
    fn size(&self) -> usize {
        self.settings.len() + self.curves.len() + usize::from(!self.look.is_empty())
    }
    /// Whether it sets every setting `other` sets (with the values the edit has,
    /// when both fit an edit whole).
    fn contains(&self, other: &Fingerprint) -> bool {
        other
            .settings
            .iter()
            .all(|(k, _)| self.settings.iter().any(|(key, _)| key == k))
            && other
                .curves
                .iter()
                .all(|(k, _)| self.curves.iter().any(|(key, _)| key == k))
            && (other.look.is_empty() || !self.look.is_empty())
    }
    /// How many of its settings `edit` has.
    fn equal(&self, edit: &Preset) -> usize {
        let settings = self
            .settings
            .iter()
            .filter(|(key, want)| edit.settings.get(key).is_some_and(|v| same(v, want)))
            .count();
        let curves = self
            .curves
            .iter()
            .filter(|(key, want)| edit.curves.get(key).is_some_and(|c| same_curve(c, want)))
            .count();
        let look = !self.look.is_empty() && self.look.eq_ignore_ascii_case(&edit.look);
        settings + curves + usize::from(look)
    }
}

impl Fingerprints {
    /// The develop presets of a library, RAWmakase's own left out; a preset that
    /// sets nothing is left out, and the same preset installed twice (same name
    /// and UUID) counts once.
    pub fn of(presets: &[Preset]) -> Self {
        let mut seen = std::collections::HashSet::new();
        let presets = presets
            .iter()
            .filter(|p| !p.photo_settings && !p.builtin)
            .map(|p| Fingerprint {
                name: p.name.clone(),
                uuid: p.settings.get("UUID").cloned().unwrap_or_default(),
                settings: p
                    .settings
                    .iter()
                    .filter(|(k, _)| {
                        !crate::xmp::METADATA.contains(&k.as_str())
                            && !NOT_COMPARED.contains(&k.as_str())
                    })
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                // Presets keep process 2010's curves (`ToneCurve`, `ToneCurveRed`…)
                // beside today's, which edits made since no longer store.
                curves: p
                    .curves
                    .iter()
                    .filter(|(k, c)| k.starts_with("ToneCurvePV2012") && !is_straight(c))
                    .map(|(k, c)| (k.clone(), c.clone()))
                    .collect(),
                look: p.look.clone(),
            })
            .filter(|f| f.size() > 0 && seen.insert((f.name.clone(), f.uuid.clone())))
            .collect();
        Self { presets }
    }
    pub fn is_empty(&self) -> bool {
        self.presets.is_empty()
    }
    /// The preset `edit`, a photo's settings, looks made with, when one fits
    /// clearly best (see the module's rules); the same preset in two groups
    /// counts once.
    pub fn infer(&self, edit: &Preset) -> Option<AppliedPreset> {
        // (share, settings, preset) of each preset that may count.
        let mut scored: Vec<(f32, usize, &Fingerprint)> = self
            .presets
            .iter()
            .filter_map(|f| {
                let (equal, size) = (f.equal(edit), f.size());
                (size >= MIN_SETTINGS || equal == size)
                    .then(|| (equal as f32 / size as f32, size, f))
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)));
        let &(share, size, best) = scored.first()?;
        if share < MIN_SHARE {
            return None;
        }
        // Every preset of another name close behind must be one the best holds
        // whole; the tolerance keeps a lead of exactly 15 points (15/20 against
        // 12/20) from failing on rounding.
        let close = scored[1..]
            .iter()
            .filter(|s| s.2.name != best.name && share - s.0 < MIN_LEAD - 1e-4);
        for &(rival, rival_size, other) in close {
            let contains = share == 1. && rival == 1. && size > rival_size && best.contains(other);
            if !contains {
                return None;
            }
        }
        Some(AppliedPreset {
            name: best.name.clone(),
            amount: None,
            uuid: best.uuid.clone(),
            inferred: true,
        })
    }
    /// [`infer`](Self::infer) for a photo's Lightroom settings `text`, which must be
    /// Camera Raw XMP; `None` for anything else, or XMP that can't be read.
    pub fn infer_text(&self, text: &str) -> Option<AppliedPreset> {
        if self.presets.is_empty()
            || !text
                .trim_start_matches('\u{feff}')
                .trim_start()
                .starts_with('<')
        {
            return None;
        }
        let edit = crate::xmp::parse(std::path::Path::new("Lightroom edit.xmp"), text).ok()?;
        self.infer(&edit)
    }
}

/// Values equal as Lightroom writes them: numbers to within rounding ("+0.30"
/// is 0.3), anything else ignoring case.
fn same(a: &str, b: &str) -> bool {
    match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        (Ok(a), Ok(b)) => (a - b).abs() < 0.006,
        _ => a.trim().eq_ignore_ascii_case(b.trim()),
    }
}

/// Whether `c` is the identity, as Lightroom writes a curve left alone.
fn is_straight(c: &ToneCurve) -> bool {
    c.points.len() == 2
        && c.points[0].iter().all(|v| v.abs() < 1e-4)
        && c.points[1].iter().all(|v| (v - 1.).abs() < 1e-4)
}

fn same_curve(a: &ToneCurve, b: &ToneCurve) -> bool {
    a.points.len() == b.points.len()
        && a.points
            .iter()
            .zip(&b.points)
            .all(|(p, q)| (p[0] - q[0]).abs() < 1e-4 && (p[1] - q[1]).abs() < 1e-4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::ns::{CRS, RDF};

    fn xmp(attrs: &str, body: &str) -> String {
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><r:RDF xmlns:r="{RDF}"><r:Description xmlns:c="{CRS}" {attrs}>{body}</r:Description></r:RDF></x:xmpmeta>"#
        )
    }
    fn curve(points: &[&str]) -> String {
        let items: String = points.iter().map(|p| format!("<r:li>{p}</r:li>")).collect();
        format!("<c:ToneCurvePV2012Red><r:Seq>{items}</r:Seq></c:ToneCurvePV2012Red>")
    }
    fn preset(name: &str, attrs: &str, body: &str) -> Preset {
        let attrs = format!(r#"c:PresetType="Normal" c:Name="{name}" {attrs}"#);
        crate::xmp::parse(
            std::path::Path::new(&format!("{name}.xmp")),
            &xmp(&attrs, body),
        )
        .unwrap()
    }

    /// Settings `K0`…: a preset or edit from its values, in order.
    fn numbered(values: &[i32]) -> String {
        values
            .iter()
            .enumerate()
            .map(|(i, v)| format!(r#"c:K{i}="{v}" "#))
            .collect()
    }

    /// Modelled on a real edit: a preset applied, then 2 of its 20 settings
    /// changed for the photo (90%), with other presets of the same look sharing
    /// about half of it.
    #[test]
    fn an_edit_adapted_after_the_preset_still_looks_like_it() {
        let v3: Vec<i32> = (0..20).collect();
        let mut v2 = v3.clone();
        v2[9..].iter_mut().for_each(|v| *v += 100);
        let mut berlin = v3.clone();
        berlin[10..].iter_mut().for_each(|v| *v -= 100);
        let fingerprints = Fingerprints::of(&[
            preset("NYC v3", &numbered(&v3), ""),
            preset("NYC v2", &numbered(&v2), ""),
            preset("Berlin", &numbered(&berlin), ""),
        ]);
        let mut edit = v3.clone();
        edit[0] += 7;
        edit[19] -= 3;
        assert_eq!(
            fingerprints
                .infer_text(&xmp(&numbered(&edit), ""))
                .map(|p| (p.name, p.inferred)),
            Some(("NYC v3".into(), true))
        );
        // Adapted until it is no closer to one than to the others: none.
        let mut far = v3;
        far[..9].iter_mut().for_each(|v| *v += 50);
        assert_eq!(fingerprints.infer_text(&xmp(&numbered(&far), "")), None);
    }

    /// A preset must fit clearly better than any other: 90% against 80% is too
    /// close to tell. A preset of few settings must fit whole, and then still
    /// lead: a curve the edit has whole, in a larger preset it fits almost whole
    /// too, names neither.
    #[test]
    fn a_preset_is_named_only_when_it_clearly_fits_best() {
        let base: Vec<i32> = (0..10).collect();
        let mut close = base.clone();
        close[0] += 100;
        let close_rival = Fingerprints::of(&[
            preset("A", &numbered(&base), ""),
            preset("B", &numbered(&close), ""),
        ]);
        let mut edit = base.clone();
        edit[9] += 1;
        assert_eq!(close_rival.infer_text(&xmp(&numbered(&edit), "")), None);

        let warm = curve(&["0, 10", "128, 140", "255, 250"]);
        let small = Fingerprints::of(&[preset("Curve", r#"c:Saturation="-10""#, &warm)]);
        assert_eq!(
            small
                .infer_text(&xmp(r#"c:Saturation="-10" c:Exposure2012="1""#, &warm))
                .map(|p| p.name),
            Some("Curve".into())
        );
        assert_eq!(
            small.infer_text(&xmp(r#"c:Saturation="5""#, &warm)),
            None,
            "a small preset fits whole or not at all"
        );
        let mut large = base.clone();
        large.push(-10);
        let both = Fingerprints::of(&[
            preset("Curve", r#"c:Saturation="-10""#, &warm),
            preset("Large", &numbered(&base[..9]), &warm),
        ]);
        let mut edit = base[..9].to_vec();
        edit[8] += 1;
        assert_eq!(
            both.infer_text(&xmp(
                &format!(r#"{}c:Saturation="-10""#, numbered(&edit)),
                &warm
            )),
            None
        );
    }

    /// RAWmakase's own presets made no Lightroom edit, and a straight tone curve
    /// is no evidence: nearly every edit has one. Neither is named.
    #[test]
    fn built_in_presets_and_straight_curves_are_no_evidence() {
        let straight = "<c:ToneCurvePV2012><r:Seq><r:li>0, 0</r:li><r:li>255, 255</r:li></r:Seq></c:ToneCurvePV2012>";
        let mut builtin = preset("Mine", &numbered(&[1, 2, 3, 4, 5]), "");
        builtin.builtin = true;
        let fingerprints = Fingerprints::of(&[preset("Linear", "", straight), builtin]);
        assert!(fingerprints.is_empty());
        let edit = xmp(&numbered(&[1, 2, 3, 4, 5]), straight);
        assert_eq!(fingerprints.infer_text(&edit), None);
    }

    /// Two presets the edit has whole name the larger only when it contains the
    /// smaller (a curve inside a look made with it); otherwise both were applied,
    /// one on top of the other, and neither is named. Every rival close behind
    /// counts, not only the first.
    #[test]
    fn whole_fits_name_the_larger_only_when_it_contains_the_smaller() {
        let warm = curve(&["0, 10", "128, 140", "255, 250"]);
        let look = preset("Look", &numbered(&[1, 2, 3, 4, 5, 6, 7]), &warm);
        let film = preset("Film", "", &warm);
        let vignette = preset("Vignette", r#"c:PostCropVignetteAmount="-20""#, "");
        let edit = xmp(
            &format!(
                r#"{}c:PostCropVignetteAmount="-20""#,
                numbered(&[1, 2, 3, 4, 5, 6, 7])
            ),
            &warm,
        );
        let contained = Fingerprints::of(&[look.clone(), film.clone()]);
        assert_eq!(
            contained.infer_text(&edit).map(|p| p.name),
            Some("Look".into())
        );
        let stacked = Fingerprints::of(&[film.clone(), vignette]);
        assert_eq!(stacked.infer_text(&edit), None, "two presets applied");
        // A third preset 10 points behind vetoes, though the first rival doesn't.
        let mut close: Vec<i32> = (1..=7).collect();
        close.extend([8, 9, 10]);
        let rival = preset("Close", &numbered(&close), &warm);
        let mut edit_values: Vec<i32> = (1..=7).collect();
        edit_values.extend([8, 9, 99]);
        let edit = xmp(&numbered(&edit_values), &warm);
        let three = Fingerprints::of(&[look, film, rival]);
        assert_eq!(three.infer_text(&edit), None);
    }

    /// A lead of exactly 15 points is enough, whatever the fractions round to.
    #[test]
    fn a_lead_of_exactly_fifteen_points_counts() {
        let best: Vec<i32> = (0..20).collect();
        let mut rival = best.clone();
        rival[12..].iter_mut().for_each(|v| *v += 100);
        let mut edit = best.clone();
        edit[15..].iter_mut().for_each(|v| *v -= 100);
        // best 15/20, rival 12/20: 15 points.
        let fingerprints = Fingerprints::of(&[
            preset("Best", &numbered(&best), ""),
            preset("Rival", &numbered(&rival), ""),
        ]);
        assert_eq!(
            fingerprints
                .infer_text(&xmp(&numbered(&edit), ""))
                .map(|p| p.name),
            Some("Best".into())
        );
    }

    /// A synthetic library checked as the real one was, with the preset each edit
    /// was made from as ground truth: presets drawn from few values, so they share
    /// many, and edits that change up to half of their preset. A preset named is
    /// always the right one, and edits changed a little are always found.
    #[test]
    fn inferred_presets_are_right_on_a_synthetic_library() {
        let mut seed = 0x2545_f491_u32;
        let mut next = move |n: u32| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed % n) as i32
        };
        let presets: Vec<Vec<i32>> = (0..40)
            .map(|_| (0..12).map(|_| next(4)).collect())
            .collect();
        let fingerprints = Fingerprints::of(
            &presets
                .iter()
                .enumerate()
                .map(|(i, v)| preset(&format!("P{i}"), &numbered(v), ""))
                .collect::<Vec<_>>(),
        );
        let (mut named, mut found_close, mut close) = (0, 0, 0);
        for (i, values) in presets.iter().enumerate() {
            for changed in 0..=6 {
                let mut edit = values.clone();
                for v in &mut edit[..changed] {
                    *v += 10;
                }
                let inferred = fingerprints.infer_text(&xmp(&numbered(&edit), ""));
                if let Some(p) = &inferred {
                    named += 1;
                    // Presets drawn alike are the same preset to the edit.
                    let same = presets[p.name[1..].parse::<usize>().unwrap()][changed..]
                        == values[changed..];
                    assert!(
                        p.name == format!("P{i}") || same,
                        "P{i} changed {changed}: {p:?}"
                    );
                }
                if changed <= 1 {
                    close += 1;
                    found_close += usize::from(inferred.is_some());
                }
            }
        }
        assert!(named > 0);
        assert_eq!(found_close, close, "edits changed a little are found");
    }

    /// The preset an edit has every setting of is found, the most specific when
    /// several fit; presets of different names that fit as well say nothing, and
    /// an edit changed since fits none.
    #[test]
    fn an_edit_looks_like_the_preset_whose_settings_it_has() {
        let warm = curve(&["0, 10", "128, 140", "255, 250"]);
        let film = preset("Film", r#"c:UUID="F1" c:Saturation="-10""#, &warm);
        let curve_only = preset("Curve", "", &warm);
        let twin_a = preset("Twin A", r#"c:Vibrance="20""#, "");
        let twin_b = preset("Twin B", r#"c:Vibrance="20""#, "");
        let fingerprints = Fingerprints::of(&[curve_only, film, twin_a, twin_b]);
        let edit = |attrs: &str, body: &str| {
            let attrs = format!(r#"c:Exposure2012="+0.50" c:AsShotTemperature="5000" {attrs}"#);
            fingerprints.infer_text(&xmp(&attrs, body))
        };
        assert_eq!(
            edit(r#"c:Saturation="-10.00""#, &warm),
            Some(AppliedPreset {
                name: "Film".into(),
                amount: None,
                uuid: "F1".into(),
                inferred: true,
            })
        );
        assert_eq!(
            edit(r#"c:Saturation="5""#, &warm).map(|p| p.name),
            Some("Curve".into())
        );
        assert_eq!(edit(r#"c:Vibrance="20""#, ""), None, "a tie names none");
        assert_eq!(edit(r#"c:Saturation="-10""#, ""), None);
        assert_eq!(fingerprints.infer_text("s = { Saturation = -10 }"), None);
    }
}
