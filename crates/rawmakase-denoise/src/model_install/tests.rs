use super::*;
use rawmakase_model_install::{RECEIPT, receipt};

fn is_installed(dir: &Path, pack: Pack) -> bool {
    rawmakase_model_install::is_installed(dir, &pack.files())
}

/// AI Denoise's model is installed only with its licence and credit beside it: a
/// folder without `LICENSE` or `NOTICE`, receipt or not, is not an install.
#[test]
fn the_denoiser_needs_its_licence_and_notice() {
    let dir = tempfile::tempdir().unwrap();
    for file in Pack::Denoise.files() {
        std::fs::File::create(dir.path().join(file.name))
            .unwrap()
            .set_len(file.size_bytes)
            .unwrap();
    }
    std::fs::write(dir.path().join(RECEIPT), receipt(&Pack::Denoise.files())).unwrap();
    assert!(is_installed(dir.path(), Pack::Denoise));
    for name in ["LICENSE", "NOTICE"] {
        let path = dir.path().join(name);
        let kept = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(!is_installed(dir.path(), Pack::Denoise), "{name}");
        std::fs::write(&path, kept).unwrap();
    }
    assert!(receipt(&Pack::Denoise.files()).contains(" LICENSE\n"));
    assert!(receipt(&Pack::Denoise.files()).contains(" NOTICE\n"));
    // Its own folder, named for the model and its version, beside where the
    // selection models were installed.
    assert!(Pack::Denoise.dir().ends_with(format!(
        "models/{}-v{}",
        rawmakase_inference::RAWNIND.id,
        rawmakase_inference::RAWNIND.version
    )));
}
