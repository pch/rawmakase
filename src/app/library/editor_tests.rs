//! Editor tests of `library`: the whole editor driven through its frames.
use crate::app::testing::*;
use crate::app::*;

/// Issue #746: with Picked shown, U in Develop hides the photo open, and the
/// arrows only work again once Develop moves to a photo still shown.
#[test]
fn unflagging_the_photo_open_in_develop_with_picked_shown_opens_one_still_shown()
-> anyhow::Result<()> {
    use crate::app::photo_metadata::Edit;
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
