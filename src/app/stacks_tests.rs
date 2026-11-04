//! Folder stacks in the app, by key as a person works: Cmd+G, S, Shift+S
//! and Cmd+Shift+G in the grid, S and Shift+S in the Loupe, and Undo and
//! Redo of each change.
use crate::app::Editor;
use crate::app::testing::editor_with_catalog;
use crate::catalog::PhotoId;
use eframe::egui::{self, Key, Modifiers, Pos2, Rect, Vec2};

fn press(e: &mut Editor, key: Key, modifiers: Modifiers) {
    let ctx = e.context.clone();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
            events: [true, false]
                .map(|pressed| egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed,
                    repeat: false,
                    modifiers,
                })
                .into(),
            ..Default::default()
        },
        |ui| e.draw(ui),
    );
    output.textures_delta.clear();
}

fn shown(e: &Editor) -> Vec<PhotoId> {
    e.library.as_ref().unwrap().shown()
}

fn stack_of(e: &Editor, id: PhotoId) -> Vec<PhotoId> {
    e.library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .stack_of(id)
        .unwrap()
}

#[test]
fn stack_keys_group_expand_reorder_and_unstack_and_undo_takes_each_back() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF", "d.RAF"])?;
    let [a, b, c, d] = ids[..] else { panic!() };
    let library = e.library.as_mut().unwrap();
    library.select(Some(a));
    library.select_range_to(c);
    press(&mut e, Key::G, Modifiers::COMMAND);
    // Collapsed under its top, the photo that was active.
    assert_eq!(stack_of(&e, b), [c, a, b]);
    assert_eq!(shown(&e), [c, d]);
    press(&mut e, Key::S, Modifiers::NONE);
    assert_eq!(shown(&e), [c, a, b, d]);
    e.library.as_mut().unwrap().select(Some(b));
    press(&mut e, Key::S, Modifiers::SHIFT);
    assert_eq!(stack_of(&e, a), [b, c, a]);
    assert_eq!(shown(&e), [b, c, a, d]);

    e.undo();
    assert_eq!(stack_of(&e, a), [c, a, b]);
    e.undo();
    assert!(stack_of(&e, a).is_empty());
    assert_eq!(shown(&e), [a, b, c, d]);
    e.redo();
    assert_eq!(stack_of(&e, a), [c, a, b]);

    e.library.as_mut().unwrap().select(Some(c));
    press(&mut e, Key::G, Modifiers::COMMAND | Modifiers::SHIFT);
    assert!(stack_of(&e, a).is_empty());
    assert_eq!(shown(&e), [a, b, c, d]);
    e.undo();
    assert_eq!(stack_of(&e, a), [c, a, b]);
    Ok(())
}

#[test]
fn the_loupe_expands_a_stack_and_moves_its_photo_to_the_top() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF", "d.RAF"])?;
    let [a, b, c, d] = ids[..] else { panic!() };
    let library = e.library.as_mut().unwrap();
    library.select(Some(a));
    library.select_range_to(c);
    press(&mut e, Key::G, Modifiers::COMMAND);
    assert_eq!(shown(&e), [c, d]);
    press(&mut e, Key::E, Modifiers::NONE);
    assert!(e.library.as_ref().unwrap().loupe_open());
    press(&mut e, Key::S, Modifiers::NONE);
    assert_eq!(shown(&e), [c, a, b, d]);
    // The next photo in the open stack, then to its top.
    press(&mut e, Key::ArrowRight, Modifiers::NONE);
    press(&mut e, Key::S, Modifiers::SHIFT);
    assert_eq!(stack_of(&e, a), [a, c, b]);
    assert!(e.library.as_ref().unwrap().loupe_open());
    e.undo();
    assert_eq!(stack_of(&e, a), [c, a, b]);
    Ok(())
}

#[test]
fn a_collection_shows_no_stacks_and_takes_no_stack_keys() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let [a, b] = ids[..] else { panic!() };
    let library = e.library.as_mut().unwrap();
    library.select(Some(a));
    library.select_range_to(b);
    press(&mut e, Key::G, Modifiers::COMMAND);
    // b was active: it is the top, shown alone.
    assert_eq!(shown(&e), [b]);
    // B, then Cmd+B: the Quick Collection, which lists photos one by one.
    press(&mut e, Key::B, Modifiers::NONE);
    press(&mut e, Key::B, Modifiers::COMMAND);
    assert_eq!(shown(&e), [b]);
    press(&mut e, Key::G, Modifiers::COMMAND | Modifiers::SHIFT);
    // Not here: the stack stays.
    assert_eq!(stack_of(&e, a), [b, a]);
    Ok(())
}

#[test]
fn keys_do_nothing_behind_the_auto_stack_window() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let [a, b] = ids[..] else { panic!() };
    let library = e.library.as_mut().unwrap();
    library.select(Some(a));
    library.select_range_to(b);
    library.stack_action(crate::app::library::StackAction::AutoStack, vec![a, b]);
    assert!(e.auto_stack_open());
    press(&mut e, Key::Num3, Modifiers::NONE);
    press(&mut e, Key::G, Modifiers::COMMAND);
    press(&mut e, Key::E, Modifiers::NONE);
    let library = e.library.as_ref().unwrap();
    assert!(library.photo(a).unwrap().rating == 0 && library.photo(b).unwrap().rating == 0);
    assert!(stack_of(&e, a).is_empty());
    assert!(!library.loupe_open());
    assert!(e.auto_stack_open());
    Ok(())
}
