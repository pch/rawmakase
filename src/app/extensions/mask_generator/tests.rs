//! The checks on either side of a mask request that need no program: the
//! coverage copied out, the image's key and the saved input identity, and
//! what errors mean to the drawer. The requests themselves run end to end in
//! `subject_mask::extension_tests`.
use super::*;
use rawmakase_extensions::protocol::viewer::Click;

#[test]
fn coverage_of_the_wrong_size_or_too_large_is_refused() {
    let bytes = vec![7u8; 12];
    assert_eq!(copy_out(Some(&bytes), 4, 3), Ok(bytes.clone()));
    // A larger mapping holds the coverage at its start.
    assert_eq!(copy_out(Some(&[1; 20]), 4, 3), Ok(vec![1; 12]));
    let short = copy_out(Some(&bytes[..11]), 4, 3);
    assert!(matches!(short, Err(Failure::Failed(why)) if why.contains("11 bytes")));
    assert!(matches!(copy_out(None, 4, 3), Err(Failure::Failed(_))));
    for (w, h) in [(MAX_SIDE + 1, 1), (1, MAX_SIDE + 1), (0, 3)] {
        assert!(
            matches!(copy_out(Some(&bytes), w, h), Err(Failure::Failed(_))),
            "{w}x{h}"
        );
    }
}

#[test]
fn all_zero_coverage_is_the_failure_it_stands_for() {
    let source = BitmapSource {
        feature: "dev.x/y".into(),
        model: "m@1".into(),
        input: "i".into(),
    };
    assert_eq!(
        keep(2, 1, vec![0, 0], Failure::NothingFound, source.clone()).err(),
        Some(Failure::NothingFound)
    );
    let kept = keep(2, 1, vec![0, 9], Failure::NothingFound, source).unwrap();
    assert_eq!((kept.width, kept.height), (2, 1));
}

#[test]
fn the_image_key_names_the_size_and_the_pixels() {
    let key = image_key(2, 1, &[1, 2, 3, 4, 5, 6]);
    assert_eq!(key.len(), 32);
    assert_ne!(key, image_key(1, 2, &[1, 2, 3, 4, 5, 6]));
    assert_ne!(key, image_key(2, 1, &[1, 2, 3, 4, 5, 7]));
    assert_eq!(key, image_key(2, 1, &[1, 2, 3, 4, 5, 6]));
}

#[test]
fn the_input_identity_covers_the_model_the_generator_and_the_input() {
    let region = Region {
        space: Space::ImageFrame,
        rect: [0., 0., 1., 1.],
        scale: 1.,
        overlap: 0,
    };
    let image = Buffer {
        handle: 0,
        bytes: 6,
        width: 2,
        height: 1,
        stride: 6,
        channels: 3,
        sample: Sample::U8,
        meaning: MaskGenerate::IMAGE,
        region,
        access: Access::Read,
    };
    let asked = MaskGenerate {
        generator: "blob".into(),
        input: None,
        coverage: Buffer {
            handle: 1,
            bytes: 2,
            stride: 2,
            channels: 1,
            meaning: Meaning::Coverage,
            access: Access::Write,
            ..image.clone()
        },
        image,
        image_key: "k".into(),
    };
    let identity = input_identity(&asked, "v1");
    assert_ne!(identity, input_identity(&asked, "v2"));
    let other = MaskGenerate {
        generator: "other".into(),
        ..asked.clone()
    };
    assert_ne!(identity, input_identity(&other, "v1"));
    let clicked = MaskGenerate {
        input: Some(ViewerInput::Prompt {
            clicks: vec![Click {
                x: 0.5,
                y: 0.5,
                positive: true,
            }],
            bounds: None,
        }),
        ..asked.clone()
    };
    assert_ne!(identity, input_identity(&clicked, "v1"));
    assert_eq!(identity, input_identity(&asked, "v1"));
}

#[test]
fn errors_map_onto_what_the_drawer_says() {
    let error = |code: ErrorCode, message: &str| failure(RemoteError::new(code, message));
    assert_eq!(error(ErrorCode::Cancelled, "x"), Failure::Cancelled);
    assert_eq!(error(ErrorCode::Disabled, "x"), Failure::Cancelled);
    let setup = NeedsSetup {
        what: "the model".into(),
        bytes: 5,
    };
    assert_eq!(failure(setup.error()), Failure::NeedsSetup(setup));
    // A setup message this version can't read shows as it is.
    assert_eq!(
        error(ErrorCode::NeedsSetup, "download it"),
        Failure::Failed("download it".into())
    );
    assert_eq!(
        error(ErrorCode::Unavailable, "no runtime"),
        Failure::RuntimeUnavailable("no runtime".into())
    );
    assert_eq!(
        error(ErrorCode::Exited, "gone"),
        Failure::Failed("gone".into())
    );
}
