//! Editor tests of the keyboard shortcuts, driven through whole frames as a
//! keyboard sends them: modifiers changed first, then the key pressed and
//! released.
use crate::app::testing::*;
use crate::app::*;
use eframe::egui::{Key, Modifiers, Pos2, Rect};

/// Cmd on macOS, Ctrl elsewhere, as egui-winit reports it, with `extra` held too.
fn command(extra: Modifiers) -> Modifiers {
    Modifiers {
        command: true,
        mac_cmd: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        ..Default::default()
    } | extra
}

/// One frame of `events` through the whole editor.
fn frame(e: &mut Editor, events: Vec<egui::Event>) {
    let ctx = e.context.clone();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1200., 800.))),
            events,
            ..Default::default()
        },
        |ui| e.draw(ui),
    );
    output.textures_delta.clear();
}

/// A key as egui-winit sends it: `key` is what it types, `physical` where it sits.
fn typed(key: Key, physical: Key, modifiers: Modifiers) -> Vec<egui::Event> {
    [true, false]
        .map(|pressed| egui::Event::Key {
            key,
            physical_key: Some(physical),
            pressed,
            repeat: false,
            modifiers,
        })
        .into()
}

/// A key pressed with `modifiers` held, then released with them, in one frame.
fn tap(e: &mut Editor, key: Key, modifiers: Modifiers) {
    tap_typed(e, key, key, modifiers);
}

fn tap_typed(e: &mut Editor, key: Key, physical: Key, modifiers: Modifiers) {
    let mut events = vec![egui::Event::ModifiersChanged(modifiers)];
    events.extend(typed(key, physical, modifiers));
    events.push(egui::Event::ModifiersChanged(Modifiers::NONE));
    frame(e, events);
}

fn library_editor(
    count: usize,
) -> anyhow::Result<(tempfile::TempDir, Editor, Vec<crate::catalog::PhotoId>)> {
    let names: Vec<String> = (0..count).map(|i| format!("{i}.RAF")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let (d, mut e, ids) = editor_with_catalog(&names)?;
    e.onboarding.visible = false;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    frame(&mut e, vec![]);
    Ok((d, e, ids))
}

#[test]
fn library_redo_key_redoes_rather_than_undoing() -> anyhow::Result<()> {
    let (_d, mut e, ids) = library_editor(2)?;
    tap(&mut e, Key::Num3, Modifiers::NONE);
    assert_eq!(rating(&e, ids[0]), 3);
    tap(&mut e, Key::Z, command(Modifiers::NONE));
    assert_eq!(rating(&e, ids[0]), 0, "Cmd+Z undoes");
    tap(&mut e, Key::Z, command(Modifiers::SHIFT));
    assert_eq!(rating(&e, ids[0]), 3, "Cmd+Shift+Z redoes");
    if !cfg!(target_os = "macos") {
        tap(&mut e, Key::Z, command(Modifiers::NONE));
        tap(&mut e, Key::Y, command(Modifiers::NONE));
        assert_eq!(rating(&e, ids[0]), 3, "Ctrl+Y redoes");
    }
    Ok(())
}

#[test]
fn a_rating_key_leaves_the_frames_other_library_keys_working() -> anyhow::Result<()> {
    let (_d, mut e, ids) = library_editor(3)?;
    let next = e.library.as_ref().unwrap().navigate(ids[0], 1);
    assert!(next.is_some());
    // A rating and a move arriving in one frame, as a fast typist sends them.
    let mut events = typed(Key::Num3, Key::Num3, Modifiers::NONE);
    events.extend(typed(Key::ArrowRight, Key::ArrowRight, Modifiers::NONE));
    frame(&mut e, events);
    assert_eq!(rating(&e, ids[0]), 3);
    assert_eq!(e.library.as_ref().unwrap().selected(), next);
    Ok(())
}

#[test]
fn a_key_typing_a_letter_is_never_also_the_key_at_its_place() -> anyhow::Result<()> {
    // Dvorak types G on the U key: Library, not unflag.
    let (_d, mut e, ids) = library_editor(2)?;
    tap(&mut e, Key::P, Modifiers::NONE);
    let flag = |e: &Editor| e.library.as_ref().unwrap().photo(ids[0]).unwrap().flag;
    assert_eq!(flag(&e), 1);
    tap_typed(&mut e, Key::G, Key::U, Modifiers::NONE);
    assert_eq!(flag(&e), 1, "G@U leaves the flag alone");
    Ok(())
}

#[test]
fn f_typed_on_the_y_key_hides_the_chrome_without_comparing() {
    let ctx = egui::Context::default();
    let (mut e, _) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        true,
    );
    e.onboarding.visible = false;
    frame(&mut e, vec![]);
    let compare = e.view.compare;
    tap_typed(&mut e, Key::F, Key::Y, Modifiers::NONE);
    assert!(e.chrome.hidden(e.module));
    assert!(e.view.compare == compare, "F@Y is no Before/After");
}

fn develop_editor() -> Editor {
    let ctx = egui::Context::default();
    let (mut e, _) = editor_with_blue_photo(
        &ctx,
        crate::app::preferences::session::Session::default(),
        true,
    );
    e.onboarding.visible = false;
    frame(&mut e, vec![]);
    e
}

#[test]
fn develop_keys_read_the_modifiers_held_for_them() {
    use crate::app::develop::state::Tool;
    // Each tap releases its modifiers within the frame, as a quick hand does.
    let mut e = develop_editor();
    tap(&mut e, Key::W, Modifiers::SHIFT);
    assert!(e.view.is(Tool::Mask), "Shift+W opens Masking");
    tap(&mut e, Key::W, Modifiers::SHIFT);
    tap(&mut e, Key::T, Modifiers::SHIFT);
    assert!(e.view.is(Tool::Guided), "Shift+T opens Guided Upright");
    tap(&mut e, Key::T, Modifiers::SHIFT);
    let compare = e.view.compare;
    tap(&mut e, Key::R, Modifiers::SHIFT);
    assert!(!e.view.is(Tool::Crop), "Shift+R is not Crop");
    assert!(e.view.compare != compare, "Shift+R is Reference View");
    tap(&mut e, Key::R, Modifiers::SHIFT);
    tap(&mut e, Key::Q, Modifiers::NONE);
    assert!(e.view.is(Tool::Remove));
    let (size, feather) = (e.view.retouch.size, e.view.retouch.feather);
    let key = if feather > 0. {
        Key::OpenBracket
    } else {
        Key::CloseBracket
    };
    tap(&mut e, key, Modifiers::SHIFT);
    assert_eq!(e.view.retouch.size, size, "Shift+bracket is not the size");
    assert!(e.view.retouch.feather != feather, "Shift+bracket feathers");
}

#[test]
fn a_virtual_copy_is_made_with_the_command_held_for_its_key() -> anyhow::Result<()> {
    let (_d, mut e, _) = library_editor(1)?;
    let count = |e: &Editor| e.library.as_ref().unwrap().session.photos().len();
    let before = count(&e);
    tap(&mut e, Key::Quote, command(Modifiers::NONE));
    assert_eq!(count(&e), before + 1);
    Ok(())
}

#[test]
fn escape_in_masking_first_drops_the_mask_not_yet_drawn() {
    use crate::app::develop::state::Tool;
    let mut e = develop_editor();
    tap(&mut e, Key::M, Modifiers::NONE);
    assert!(e.view.is(Tool::Mask) && e.view.masking.is_placing());
    tap(&mut e, Key::Escape, Modifiers::NONE);
    assert!(!e.view.masking.is_placing(), "Escape drops the linear mask");
    assert!(e.view.is(Tool::Mask), "and leaves Masking open");
    tap(&mut e, Key::Escape, Modifiers::NONE);
    assert!(e.view.is(Tool::None), "the next Escape closes Masking");
    // Escape with a modifier is not Escape (#963): it closed Masking at once.
    tap(&mut e, Key::M, Modifiers::NONE);
    for modifiers in [command(Modifiers::NONE), Modifiers::ALT] {
        tap(&mut e, Key::Escape, modifiers);
        assert!(e.view.is(Tool::Mask) && e.view.masking.is_placing());
    }
}

#[test]
fn a_keypad_arrow_moves_without_labelling() -> anyhow::Result<()> {
    // With Num Lock off the keypad's 6 types ArrowRight at the place of Num6.
    let (_d, mut e, ids) = library_editor(3)?;
    let next = e.library.as_ref().unwrap().navigate(ids[0], 1);
    tap_typed(&mut e, Key::ArrowRight, Key::Num6, Modifiers::NONE);
    let library = e.library.as_ref().unwrap();
    assert_eq!(library.selected(), next);
    assert!(
        library.photo(ids[0]).unwrap().label.is_empty(),
        "no Red label"
    );
    Ok(())
}

#[test]
fn shift_o_then_o_in_one_frame_still_turns_the_mask_overlay() {
    use crate::app::develop::state::Tool;
    let mut e = develop_editor();
    tap(&mut e, Key::W, Modifiers::SHIFT);
    assert!(e.view.is(Tool::Mask));
    let overlay = e.view.masking.overlay;
    let mut events = vec![egui::Event::ModifiersChanged(Modifiers::SHIFT)];
    events.extend(typed(Key::O, Key::O, Modifiers::SHIFT));
    events.push(egui::Event::ModifiersChanged(Modifiers::NONE));
    events.extend(typed(Key::O, Key::O, Modifiers::NONE));
    frame(&mut e, events);
    assert!(e.view.masking.overlay != overlay);
}

#[test]
fn a_label_typed_as_a_symbol_is_not_also_that_symbols_key() -> anyhow::Result<()> {
    // German Shift+7 types `/` on the 7 key: Yellow and on to the next photo,
    // not also `/`, which deselects in the grid.
    let (_d, mut e, ids) = library_editor(3)?;
    let next = e.library.as_ref().unwrap().navigate(ids[0], 1);
    tap_typed(&mut e, Key::Slash, Key::Num7, Modifiers::SHIFT);
    let library = e.library.as_ref().unwrap();
    assert_eq!(library.photo(ids[0]).unwrap().label, "Yellow");
    assert_eq!(library.selected(), next);
    Ok(())
}
