use super::*;
use crate::app::shell::commands::registry::bindings as b;
use std::collections::BTreeMap;

fn user(entries: &[(&'static str, &[&str])]) -> Effective {
    let user: BTreeMap<&'static str, Vec<Chord>> = entries
        .iter()
        .map(|(id, keys)| {
            let chords = keys
                .iter()
                .map(|k| Chord::parse(k).expect("a chord"))
                .collect();
            (*id, chords)
        })
        .collect();
    Effective::new(user)
}
/// The clashes between the commands `x` and `y`.
fn between(map: &Effective, macos: bool, x: &str, y: &str) -> Vec<Clash> {
    clashes(map, macos)
        .into_iter()
        .filter(|c| (c.a.command, c.b.command) == (x, y) || (c.a.command, c.b.command) == (y, x))
        .collect()
}
fn describe(c: &Clash) -> String {
    format!(
        "{} ({:?}) and {} ({:?}) on {}",
        c.a.command, c.a.context, c.b.command, c.b.context, c.chord
    )
}

#[test]
fn the_defaults_have_no_conflicts_on_any_platform() {
    for macos in [true, false] {
        let conflicts: Vec<String> = clashes(&Effective::default(), macos)
            .iter()
            .filter(|c| c.conflict)
            .map(describe)
            .collect();
        assert_eq!(conflicts, Vec::<String>::new(), "macOS: {macos}");
    }
}

#[test]
fn a_tool_shadows_the_keys_it_shares_with_develop_and_everywhere() {
    let shadows: Vec<(&str, &str)> = clashes(&Effective::default(), false)
        .iter()
        .filter(|c| !c.conflict)
        .map(|c| (c.a.command, c.b.command))
        .collect();
    for pair in [
        ("reject", "crop:swap_orientation"),
        ("rating:lower", "brush:smaller"),
        ("rating:raise", "brush:larger"),
        ("tool:close", "mask:back"),
    ]
    .iter()
    {
        assert!(
            shadows.contains(pair) || shadows.contains(&(pair.1, pair.0)),
            "{pair:?} in {shadows:?}"
        );
    }
}

#[test]
fn two_commands_on_one_key_in_one_context_conflict() {
    // Develop's clipping and a Basic key, read in different steps.
    let map = user(&[("clipping", &["F9"]), ("basic:next", &["F9"])]);
    let conflicts: Vec<String> = clashes(&map, false)
        .iter()
        .filter(|c| c.conflict)
        .map(describe)
        .collect();
    assert_eq!(
        conflicts,
        ["clipping (Develop) and basic:next (Develop) on F9"]
    );
}

#[test]
fn one_key_in_contexts_never_active_together_is_no_conflict() {
    let map = user(&[("grid:cell_style", &["F9"]), ("clipping", &["F9"])]);
    assert!(between(&map, false, "grid:cell_style", "clipping").is_empty());
}

#[test]
fn modifiers_count_once_variants_and_punctuation_are_free() {
    // Shift goes on to the next photo after a rating: Shift+F9 is still it.
    let map = user(&[("rating:3", &["F9"]), ("crop", &["Shift+F9"])]);
    let found = between(&map, false, "rating:3", "crop");
    assert_eq!(found.len(), 1);
    assert!(!found[0].conflict, "Everywhere under Develop");
    // Cmd+F9 is another chord.
    let map = user(&[("rating:3", &["F9"]), ("crop", &["Cmd+F9"])]);
    assert!(between(&map, false, "rating:3", "crop").is_empty());
    // On punctuation, Shift is free unless named: German Cmd+/ holds Shift.
    let map = user(&[
        ("virtual_copy", &["Cmd+;"]),
        ("preferences", &["Cmd+Shift+;"]),
    ]);
    let found = between(&map, false, "virtual_copy", "preferences");
    assert_eq!(found.iter().filter(|c| c.conflict).count(), 1);
}

#[test]
fn ctrl_is_a_modifier_of_its_own_only_on_a_mac() {
    use eframe::egui::Key;
    // A Mac's Control+F9 against Cmd+F9.
    let map = Effective::new(BTreeMap::from([
        ("crop", vec![Chord::new(Key::F9, Mods::CTRL)]),
        ("clipping", vec![Chord::new(Key::F9, Mods::CMD)]),
    ]));
    assert!(between(&map, true, "crop", "clipping").is_empty());
    // Elsewhere no press holds Ctrl apart from Cmd.
    assert!(between(&map, false, "crop", "clipping").is_empty());
    let both = Effective::new(BTreeMap::from([
        ("crop", vec![Chord::new(Key::F9, Mods::CTRL)]),
        ("clipping", vec![Chord::new(Key::F9, Mods::CTRL)]),
    ]));
    assert_eq!(between(&both, true, "crop", "clipping").len(), 1);
}

#[test]
fn a_clash_names_both_bindings_and_the_chord() {
    let map = user(&[("clipping", &["F9"]), ("basic:next", &["F9"])]);
    let found = between(&map, false, "clipping", "basic:next");
    let clash = found.first().expect("a clash");
    assert!(std::ptr::eq(clash.a, &b::CLIPPING) && std::ptr::eq(clash.b, &b::BASIC_NEXT));
    assert_eq!(clash.chord.to_string(), "F9");
}
