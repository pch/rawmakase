//! Which bindings share a chord where both can take a press.
//!
//! Two bindings clash when their contexts can be active together and some
//! press is a chord of both, once their variants and the punctuation rule
//! ([`Chord::free`]) are counted. On one level of precedence that is a
//! conflict: a press of it runs neither command. Across levels it is
//! shadowing: the higher one (a tool over Develop, a Library view over the
//! Library, a module over everywhere) takes the press, as Crop takes X.
use super::rules::level;
use super::{Binding, Chord, Default, Effective, Mods};
use crate::app::shell::commands::registry::bindings;

/// Two bindings on one chord.
#[derive(Clone, Copy)]
pub(in crate::app) struct Clash {
    pub a: &'static Binding,
    pub b: &'static Binding,
    /// `a`'s chord that `b` overlaps.
    pub chord: Chord,
    /// On one level: a press runs neither. Otherwise the higher one runs.
    pub conflict: bool,
}

/// Every pair of bindings of different commands that share a chord in
/// `map`, on a Mac (`macos`) or elsewhere, each pair once.
pub(in crate::app) fn clashes(map: &Effective, macos: bool) -> Vec<Clash> {
    let all = bindings::ALL;
    let chords: Vec<Vec<Default>> = all
        .iter()
        .map(|b| {
            map.defaults_of(b)
                .iter()
                .filter(|d| d.exists_on(macos))
                .copied()
                .collect()
        })
        .collect();
    let mut out = Vec::new();
    for (i, a) in all.iter().enumerate() {
        for (j, b) in all.iter().enumerate().skip(i + 1) {
            if a.command == b.command || !a.context.overlaps(b.context) {
                continue;
            }
            let shared = chords[i].iter().find_map(|da| {
                chords[j]
                    .iter()
                    .find_map(|db| overlap(da, a.variants, db, b.variants, macos))
            });
            if let Some(chord) = shared {
                out.push(Clash {
                    a,
                    b,
                    chord,
                    conflict: level(a.context) == level(b.context),
                });
            }
        }
    }
    out
}

/// A chord of `a` that some press shares with a chord of `b`: the same key,
/// with modifiers both accept.
fn overlap(a: &Default, va: Mods, b: &Default, vb: Mods, macos: bool) -> Option<Chord> {
    a.chords().find(|ca| {
        b.chords().any(|cb| {
            let (free_a, free_b) = (ca.free(va, a.rejects), cb.free(vb, b.rejects));
            ca.key == cb.key
                && held_sets(macos).any(|held| {
                    ca.accepts(ca.key, held, free_a) && cb.accepts(cb.key, held, free_b)
                })
        })
    })
}

/// Every set of modifiers a press can hold: Ctrl apart from Cmd only on a Mac.
fn held_sets(macos: bool) -> impl Iterator<Item = Mods> {
    let one = [Mods::CMD, Mods::CTRL, Mods::ALT, Mods::SHIFT];
    (0..16u8)
        .map(move |bits| {
            one.iter()
                .enumerate()
                .filter(|(i, _)| bits & (1 << i) != 0)
                .fold(Mods::NONE, |all, (_, m)| all.with(*m))
        })
        .filter(move |held| macos || !held.contains(Mods::CTRL))
}

#[cfg(test)]
mod tests;
