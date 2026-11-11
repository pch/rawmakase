//! The pinned contract of the selection models.
//!
//! Everything a pre/post-processing step or an installer needs to know about them lives
//! here; nothing else in the crate hard-codes a size, a tensor name or a normalization
//! constant. Two models work together for Subject: [`SUBJECT`] (Segment Anything 2) draws
//! crisp object masks and answers clicks, and [`PANOPTIC`] (DETR) says where the photo's
//! people and animals are. Sky is a model of its own, [`SKY`] (UperNet), installed apart.

use rawmakase_inference::ModelFile;

/// RAWmakase's mirror of the model files on Hugging Face, and the commit they are
/// pinned to (see its README and NOTICE for provenance and licence).
pub const MIRROR: &str =
    "https://huggingface.co/pch/rawmakase-models/resolve/5748347a4c11d29b1c747859a4c24883a01215ac";

/// The interactive model's contract: identity, artifacts, tensors and preprocessing.
#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    pub id: &'static str,
    /// Version of this contract (bumped when anything below changes in a way that can
    /// change the produced mask, including the pre/post-processing code).
    pub version: u32,
    /// Every file the model needs. A graph's weights sit in the `_data` file beside it
    /// and are read from there by name.
    pub files: &'static [ModelFile],
    pub encoder_file: &'static str,
    pub decoder_file: &'static str,
    /// Square side the image encoder is fed: the photo is stretched to it.
    pub input_size: usize,
    /// Side of the low-resolution mask logits the decoder returns.
    pub mask_size: usize,
    /// Per-channel mean/std applied to RGB scaled to 0..=1: `(v - mean) / std`.
    pub mean: [f32; 3],
    pub std: [f32; 3],
    /// Version of the pre/post-processing code paths in this crate (resampling filters,
    /// edge refinement, automatic selection). Part of any cache key.
    pub processing_version: u32,
    pub license: &'static str,
    pub attribution: &'static str,
}

/// The panoptic model's contract: one graph that labels the photo's people and animals,
/// at any size.
#[derive(Debug, Clone, Copy)]
pub struct PanopticSpec {
    pub id: &'static str,
    pub file: ModelFile,
    /// The photo is scaled to this long edge, keeping its proportions.
    pub long_edge: usize,
    pub mean: [f32; 3],
    pub std: [f32; 3],
    /// Class ids (COCO's) the model labels people and animals with.
    pub people: (usize, usize),
    pub animals: (usize, usize),
    /// A query counts as an object when the model is at least this sure of its class.
    pub confidence: f32,
    pub license: &'static str,
    pub attribution: &'static str,
}

/// SAM 2.1 (Hiera small), Meta's Segment Anything Model 2, in the ONNX export of the
/// Hugging Face `onnx-community` (repository `onnx-community/sam2.1-hiera-small-ONNX`, at
/// an immutable commit): an image encoder run once per photo, and a prompt encoder with
/// mask decoder run for every prompt.
pub const SUBJECT: ModelSpec = ModelSpec {
    id: "sam2.1-hiera-small",
    version: 1,
    files: &[
        ModelFile {
            name: "vision_encoder.onnx",
            size_bytes: 467_440,
            sha256: "aacf1f7137bb6fffcf6bf166abcfabe28f57a76059254f3fb611c4a64a208119",
            url: concat!(
                "https://huggingface.co/pch/rawmakase-models/resolve/5748347a4c11d29b1c747859a4c24883a01215ac/",
                "sam2.1-hiera-small/vision_encoder.onnx"
            ),
            fallback: "https://huggingface.co/onnx-community/sam2.1-hiera-small-ONNX/resolve/a7df49d8de14b9d2e4504d1687b0d568f905fd8d/onnx/vision_encoder.onnx",
        },
        ModelFile {
            name: "vision_encoder.onnx_data",
            size_bytes: 162_476_288,
            sha256: "260fd1f0a34e72a3dc79a739e563b4facc0ba75504818b433a1f808e66637456",
            url: concat!(
                "https://huggingface.co/pch/rawmakase-models/resolve/5748347a4c11d29b1c747859a4c24883a01215ac/",
                "sam2.1-hiera-small/vision_encoder.onnx_data"
            ),
            fallback: "https://huggingface.co/onnx-community/sam2.1-hiera-small-ONNX/resolve/a7df49d8de14b9d2e4504d1687b0d568f905fd8d/onnx/vision_encoder.onnx_data",
        },
        ModelFile {
            name: "prompt_encoder_mask_decoder.onnx",
            size_bytes: 213_114,
            sha256: "079c59b261f723ff5c6a125e69b0170a957b21c58738c28d2b0394ecd0587d7f",
            url: concat!(
                "https://huggingface.co/pch/rawmakase-models/resolve/5748347a4c11d29b1c747859a4c24883a01215ac/",
                "sam2.1-hiera-small/prompt_encoder_mask_decoder.onnx"
            ),
            fallback: "https://huggingface.co/onnx-community/sam2.1-hiera-small-ONNX/resolve/a7df49d8de14b9d2e4504d1687b0d568f905fd8d/onnx/prompt_encoder_mask_decoder.onnx",
        },
        ModelFile {
            name: "prompt_encoder_mask_decoder.onnx_data",
            size_bytes: 20_958_208,
            sha256: "f9e59a584ab8ced21fa812c211bc01084204db1c9e92a5ef4fb3a49972b4e864",
            url: concat!(
                "https://huggingface.co/pch/rawmakase-models/resolve/5748347a4c11d29b1c747859a4c24883a01215ac/",
                "sam2.1-hiera-small/prompt_encoder_mask_decoder.onnx_data"
            ),
            fallback: "https://huggingface.co/onnx-community/sam2.1-hiera-small-ONNX/resolve/a7df49d8de14b9d2e4504d1687b0d568f905fd8d/onnx/prompt_encoder_mask_decoder.onnx_data",
        },
    ],
    encoder_file: "vision_encoder.onnx",
    decoder_file: "prompt_encoder_mask_decoder.onnx",
    input_size: 1024,
    mask_size: 256,
    mean: [0.485, 0.456, 0.406],
    std: [0.229, 0.224, 0.225],
    processing_version: 5,
    license: "Apache-2.0 (facebook/sam2.1-hiera-small, weights and code). The ONNX export is \
              the Hugging Face onnx-community's conversion of those weights.",
    attribution: "Segment Anything Model 2 (SAM 2), Ravi et al., Meta AI, 2024, \
                  https://github.com/facebookresearch/sam2, Apache-2.0; ONNX export by \
                  onnx-community on Hugging Face.",
};

impl PanopticSpec {
    /// Whether `class` is a person or an animal.
    pub fn is_subject(&self, class: usize) -> bool {
        [self.people, self.animals]
            .iter()
            .any(|(first, last)| (*first..=*last).contains(&class))
    }
}

/// DETR ResNet-50 panoptic (Facebook AI Research, trained on COCO), in the ONNX export
/// of Xenova's Transformers.js conversion (half precision). It says where the people and
/// animals are; SAM 2 draws their outlines.
pub const PANOPTIC: PanopticSpec = PanopticSpec {
    id: "detr-resnet-50-panoptic",
    file: ModelFile {
        name: "detr-panoptic-fp16.onnx",
        size_bytes: 86_559_030,
        sha256: "afd9f02d864302d690356fd4bfcb2feed2397a1190bf46a7306cb430464d734a",
        url: concat!(
            "https://huggingface.co/pch/rawmakase-models/resolve/5748347a4c11d29b1c747859a4c24883a01215ac/",
            "detr-resnet-50-panoptic/detr-panoptic-fp16.onnx"
        ),
        fallback: "https://huggingface.co/Xenova/detr-resnet-50-panoptic/resolve/ea24b2d4e0bfae31f0a1299ba3fb892a2df064de/onnx/model_fp16.onnx",
    },
    long_edge: 800,
    mean: [0.485, 0.456, 0.406],
    std: [0.229, 0.224, 0.225],
    people: (1, 1),
    animals: (16, 25),
    confidence: 0.85,
    license: "Apache-2.0 (facebook/detr-resnet-50-panoptic, trained on COCO). The ONNX export \
              is Xenova's conversion of those weights; its card declares no license of its own.",
    attribution: "End-to-End Object Detection with Transformers (DETR), Carion et al., Facebook \
                  AI Research, 2020, https://github.com/facebookresearch/detr, Apache-2.0; \
                  ONNX export by Xenova on Hugging Face.",
};

/// The sky model's contract: one graph from a photo of any size to the probability, at
/// each of its pixels, that it is sky.
#[derive(Debug, Clone, Copy)]
pub struct SkySpec {
    pub id: &'static str,
    /// Version of this contract, as [`ModelSpec::version`].
    pub version: u32,
    pub file: ModelFile,
    /// The photo is scaled to this long edge, keeping its proportions.
    pub long_edge: usize,
    /// Neither side goes below this: the network halves it five times.
    pub min_side: usize,
    pub mean: [f32; 3],
    pub std: [f32; 3],
    /// Version of the pre/post-processing code paths, as [`ModelSpec::processing_version`].
    pub processing_version: u32,
    pub license: &'static str,
    pub attribution: &'static str,
}

/// UperNet with a ConvNeXt base backbone, trained on ADE20K, exported to ONNX by
/// `scripts/export_upernet_sky.py` from `openmmlab/upernet-convnext-base` with half-precision
/// weights and its adaptive pooling written so the graph takes any size. There is no upstream
/// export to fall back on, so both sources are the mirror.
pub const SKY: SkySpec = SkySpec {
    id: "upernet-convnext-base",
    version: 1,
    file: ModelFile {
        name: "upernet-convnext-base-sky-fp16.onnx",
        size_bytes: 242_162_020,
        sha256: "ed75b2ad81ad209949c9580071ed486a72a423037822a49faa4e98991a43b891",
        url: SKY_URL,
        fallback: SKY_URL,
    },
    long_edge: 1024,
    min_side: 64,
    mean: [0.485, 0.456, 0.406],
    std: [0.229, 0.224, 0.225],
    processing_version: 5,
    license: "MIT (openmmlab/upernet-convnext-base's card). The checkpoint was trained on \
              ADE20K, whose images MIT offers for non-commercial research and education.",
    attribution: "UperNet (Xiao et al., 2018) with ConvNeXt (Liu et al., Meta AI, 2022), trained \
                  on ADE20K (Zhou et al., MIT CSAIL) by OpenMMLab; ONNX export by RAWmakase.",
};

/// The mirror's commit that added the sky model: later than [`MIRROR`], whose files stay
/// pinned where they are.
const SKY_URL: &str = "https://huggingface.co/pch/rawmakase-models/resolve/b727de8d854f7d5c50396323abe1a3b12e743954/upernet-convnext-base/upernet-convnext-base-sky-fp16.onnx";

/// Every file Subject and Background need, in one folder.
pub fn all_files() -> Vec<ModelFile> {
    SUBJECT
        .files
        .iter()
        .copied()
        .chain([PANOPTIC.file])
        .collect()
}

/// Bytes of [`all_files`].
pub fn total_bytes() -> u64 {
    all_files().iter().map(|f| f.size_bytes).sum()
}

/// Every file Sky needs, in a folder of its own: having Subject's models does not oblige
/// anyone to fetch this one, nor the other way round.
pub fn sky_files() -> Vec<ModelFile> {
    vec![SKY.file]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests_are_lowercase_sha256_hex_and_urls_are_pinned() {
        for file in all_files() {
            assert_eq!(file.sha256.len(), 64, "{}", file.name);
            assert!(
                file.sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            );
            for url in [file.url, file.fallback] {
                assert!(
                    url.starts_with("https://") && !url.contains("/main/"),
                    "{url}"
                );
            }
            assert!(file.url.starts_with(MIRROR), "{}", file.url);
        }
        assert!(SUBJECT.files.iter().any(|f| f.name == SUBJECT.encoder_file));
        assert!(SUBJECT.files.iter().any(|f| f.name == SUBJECT.decoder_file));
        assert_eq!(total_bytes(), 184_115_050 + 86_559_030);
        // Sky is apart: nobody who has Subject's models has to fetch it.
        assert!(!all_files().iter().any(|f| f.name == SKY.file.name));
        assert_eq!(SKY.file.sha256.len(), 64);
        let commit = SKY_URL
            .strip_prefix("https://huggingface.co/pch/rawmakase-models/resolve/")
            .and_then(|rest| rest.split('/').next())
            .unwrap();
        assert!(commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()));
        // The model never shares a folder or a file with the selection models.
        let files = rawmakase_inference::manifest::denoise_files();
        assert!(
            !all_files()
                .iter()
                .any(|f| files.iter().any(|d| d.name == f.name))
        );
    }
}
