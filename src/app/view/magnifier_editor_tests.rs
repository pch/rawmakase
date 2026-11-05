//! Editor tests of `magnifier`: the whole editor driven through its frames.
use crate::app::*;

/// In the Library's Loupe, a RAW (drawn through Develop's pipeline) can be
/// magnified at Fit, not zoomed in; a point chosen on the minimap of a photo the
/// Loupe draws itself zooms in there.
#[test]
fn the_librarys_loupe_magnifies_raws_at_fit_and_its_minimap_zooms() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("photos.rawmakase");
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    for name in ["a.ARW", "b.ARW"] {
        std::fs::write(photos.join(name), name)?;
    }
    crate::catalog::Catalog::create(&path)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    e.library = Some(Box::new(library::Library::load(&path, ctx)?));
    e.module = Module::Library;
    e.library.as_mut().unwrap().open_loupe();
    assert!(e.library.as_ref().unwrap().loupe_develops().is_some());
    assert_eq!(e.magnifier_blocked(), None);
    e.toggle_magnifier();
    assert!(e.view.magnifier.on);
    e.library_inspect([0.25, 0.75]);
    assert!(e.view.zoom.on);
    assert!(e.view.glide.is_some());
    assert!(e.magnifier_blocked().is_some());
    Ok(())
}
