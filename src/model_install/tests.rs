use super::*;
use rawmakase_model_install::{RECEIPT, receipt};

fn is_installed(dir: &Path, pack: Pack) -> bool {
    rawmakase_model_install::is_installed(dir, &pack.files())
}

#[test]
fn each_pack_has_a_folder_named_for_its_model_and_version() {
    assert!(Pack::Subject.dir().ends_with(format!(
        "models/{}-v{}",
        rawmakase_inference::SUBJECT.id,
        rawmakase_inference::SUBJECT.version
    )));
    assert!(Pack::Sky.dir().ends_with(format!(
        "models/{}-v{}",
        rawmakase_inference::SKY.id,
        rawmakase_inference::SKY.version
    )));
    assert!(Pack::Subject.megabytes() > 250 && Pack::Sky.megabytes() > 200);
}

/// A folder holding `pack`'s files at their sizes, with its receipt.
fn fake_install(pack: Pack) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for file in pack.files() {
        let f = std::fs::File::create(dir.path().join(file.name)).unwrap();
        f.set_len(file.size_bytes).unwrap();
    }
    std::fs::write(dir.path().join(RECEIPT), receipt(&pack.files())).unwrap();
    dir
}

/// What an install from before Sky had its own model holds: it must keep working after
/// the update, without fetching anything.
#[test]
fn subjects_models_stay_installed_without_the_sky_model() {
    let dir = fake_install(Pack::Subject);
    assert!(is_installed(dir.path(), Pack::Subject));
    assert!(!is_installed(dir.path(), Pack::Sky));
    let receipt_of_the_old_install = [
        "aacf1f7137bb6fffcf6bf166abcfabe28f57a76059254f3fb611c4a64a208119 467440 vision_encoder.onnx",
        "260fd1f0a34e72a3dc79a739e563b4facc0ba75504818b433a1f808e66637456 162476288 vision_encoder.onnx_data",
        "079c59b261f723ff5c6a125e69b0170a957b21c58738c28d2b0394ecd0587d7f 213114 prompt_encoder_mask_decoder.onnx",
        "f9e59a584ab8ced21fa812c211bc01084204db1c9e92a5ef4fb3a49972b4e864 20958208 prompt_encoder_mask_decoder.onnx_data",
        "afd9f02d864302d690356fd4bfcb2feed2397a1190bf46a7306cb430464d734a 86559030 detr-panoptic-fp16.onnx",
        "",
    ]
    .join("\n");
    assert_eq!(receipt(&Pack::Subject.files()), receipt_of_the_old_install);
    let sky = fake_install(Pack::Sky);
    assert!(is_installed(sky.path(), Pack::Sky));
    assert!(!is_installed(sky.path(), Pack::Subject));
}

#[test]
fn a_model_is_installed_only_when_every_file_is_there_with_its_size_and_checked() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!is_installed(dir.path(), Pack::Subject));
    for file in all_files() {
        let f = std::fs::File::create(dir.path().join(file.name)).unwrap();
        f.set_len(file.size_bytes).unwrap();
    }
    // The right sizes are not enough without the installer's receipt.
    assert!(!is_installed(dir.path(), Pack::Subject));
    std::fs::write(dir.path().join(RECEIPT), receipt(&Pack::Subject.files())).unwrap();
    assert!(is_installed(dir.path(), Pack::Subject));
    std::fs::File::create(dir.path().join(all_files()[1].name))
        .unwrap()
        .set_len(5)
        .unwrap();
    assert!(!is_installed(dir.path(), Pack::Subject));
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
    // Its own folder: none of the selection models' files.
    assert_ne!(Pack::Denoise.dir(), Pack::Subject.dir());
}
