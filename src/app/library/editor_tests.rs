//! Editor tests of `library`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;

/// Issue #746: with Picked shown, U in Develop hides the photo open, and the
/// arrows only work again once Develop moves to a photo still shown.
#[test]
fn unflagging_the_photo_open_in_develop_with_picked_shown_opens_one_still_shown()
-> anyhow::Result<()> {
    use crate::app::library::photo_metadata::Edit;
    let (_dir, _ctx, mut editor, [a, b]) = two_photo_editor()?;
    let library = editor.library.as_mut().unwrap();
    library.edit_photos(&[a, b], Edit::Flag(1), false)?;
    let mut layout = library.layout();
    layout.flags = vec![1];
    layout.filters_off = false;
    library.apply_layout(&layout);
    editor.module = Module::Develop;
    editor.develop_catalog_photo(b);
    assert_eq!(editor.document.catalog_photo, Some(b));
    // U, as the Develop shortcut sends it.
    assert!(editor.command_metadata(Edit::Flag(0), None, false).is_ok());
    assert_eq!(editor.library.as_ref().unwrap().photo(b).unwrap().flag, 0);
    assert_eq!(editor.document.catalog_photo, Some(a));
    assert_eq!(editor.library.as_ref().unwrap().selected(), Some(a));
    assert_eq!(editor.module, Module::Develop);
    // The Library moves its selection to B, picked again, but does not
    // open Develop.
    editor.module = Module::Library;
    assert!(
        editor
            .command_metadata(Edit::Flag(1), Some(b), false)
            .is_ok()
    );
    editor.library.as_mut().unwrap().make_active(a);
    assert!(
        editor
            .command_metadata(Edit::Flag(0), Some(a), false)
            .is_ok()
    );
    assert_eq!(editor.library.as_ref().unwrap().selected(), Some(b));
    assert_eq!(editor.module, Module::Library);
    assert_eq!(editor.document.catalog_photo, Some(a));
    Ok(())
}

/// What the session file says, as last written.
fn saved_session(path: &std::path::Path) -> crate::app::preferences::session::Session {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// The Library's place is remembered as soon as it changes, with the layout shown then
/// even while a drag or typing is still changing it; a change of layout alone waits
/// until it settles; without a Library nothing is remembered.
#[test]
fn the_place_is_remembered_at_once_and_a_layout_alone_once_it_settles() -> anyhow::Result<()> {
    use crate::app::library::views::View;
    use crate::app::shell::workspace::LayoutEdit;
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let session = d.path().join("session.json");
    e.session.set_file(Some(session.clone()));
    e.library.as_mut().unwrap().select(Some(ids[0]));
    e.remember_place(LayoutEdit::Settled);
    assert_eq!(saved_session(&session).selected_photo, Some(ids[0]));

    // A new place while the layout changes: both are kept, the layout as shown.
    let library = e.library.as_mut().unwrap();
    library.set_view(View::Loupe);
    library.select(Some(ids[1]));
    e.remember_place(LayoutEdit::Changing);
    let saved = saved_session(&session);
    assert_eq!(saved.selected_photo, Some(ids[1]));
    assert_eq!(saved.library_layout.view, "loupe");

    // The layout alone: not while it changes, then once it settles.
    e.library.as_mut().unwrap().set_view(View::Grid);
    e.remember_place(LayoutEdit::Changing);
    assert_eq!(saved_session(&session).library_layout.view, "loupe");
    e.remember_place(LayoutEdit::Settled);
    assert_eq!(saved_session(&session).library_layout.view, "grid");

    // Without a Library, what was remembered stays.
    std::fs::remove_file(&session)?;
    e.library = None;
    e.remember_place(LayoutEdit::Settled);
    assert!(!session.exists());
    Ok(())
}
