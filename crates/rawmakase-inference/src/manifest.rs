//! The pinned contract of the selection models.
//!
//! Everything a pre/post-processing step or an installer needs to know about them lives
//! here; nothing else in the crate hard-codes a size, a tensor name or a normalization
//! constant. Two models work together for Subject: [`SUBJECT`] (Segment Anything 2) draws
//! crisp object masks and answers clicks, and [`PANOPTIC`] (DETR) says where the photo's
//! people and animals are. Sky is a model of its own, [`SKY`] (UperNet), installed apart.

/// One file of the models, as stored under their folder.
#[derive(Debug, Clone, Copy)]
pub struct ModelFile {
    pub name: &'static str,
    pub size_bytes: u64,
    pub sha256: &'static str,
    /// Where it is fetched from: RAWmakase's own mirror, pinned to an immutable commit,
    /// and checked against `sha256` whatever the host sends.
    pub url: &'static str,
    /// The repository the mirror copied it from, pinned the same way: tried when the
    /// mirror cannot be reached. The same bytes, so the same checksum.
    pub fallback: &'static str,
}

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

/// AI Denoise's model contract: RawNIND UtNet2 (Bayer), which denoises and demosaics a
/// Bayer mosaic in one pass. Everything the tiler, the gain fit and the installer need.
#[derive(Debug, Clone, Copy)]
pub struct DenoiseSpec {
    pub id: &'static str,
    /// Version of the adapter: the model file, the packing, tiling, mirror padding, gain
    /// rule and assembly. Part of every cache key, so a change here recomputes results.
    pub version: u32,
    /// The ONNX graph.
    pub model: ModelFile,
    /// The licence, the credit (`NOTICE`, the bytes of [`DenoiseSpec::attribution`] and a
    /// line feed) and the model card, installed and verified with the model: an installed
    /// model always carries its credit.
    pub notices: [ModelFile; 3],
    pub input: &'static str,
    pub output: &'static str,
    /// Packed side of a tile: the graph's input is `[1, 4, tile, tile]` (R, G1, G2, B
    /// planes of an RGGB mosaic) and its output `[1, 3, 2 × tile, 2 × tile]`.
    pub tile: usize,
    /// Packed pixels of context on each side of a tile's kept core.
    pub overlap: usize,
    /// The providers the model may run on. darktable pins it to CoreML's CPU units, as its
    /// activations overflow half precision; ONNX Runtime's CoreML ML Program runs in
    /// single precision on the GPU, which phase 0 measured equal to the CPU (see
    /// docs/rendering-quality.md#ai-denoise).
    pub providers: &'static [crate::runtime::Provider],
    pub license: &'static str,
    /// The credit, shown before the download, in the Detail panel and in About, and
    /// installed as `NOTICE`: the one text every place shows.
    pub attribution: &'static str,
}

/// The mirror's commit that holds `rawnind-utnet2-bayer/`.
pub const DENOISE_MIRROR: &str =
    "https://huggingface.co/pch/rawmakase-models/resolve/ce76dbfa066e6eafb1162048613534ae204346b3";
/// The same bytes as release assets of RAWmakase, tried when the mirror cannot be reached.
pub const DENOISE_RELEASE: &str =
    "https://github.com/pch/rawmakase/releases/download/models-rawnind-utnet2-bayer-1";

macro_rules! denoise_file {
    ($name:literal, $size:expr, $sha:expr) => {
        ModelFile {
            name: $name,
            size_bytes: $size,
            sha256: $sha,
            url: concat!(
                "https://huggingface.co/pch/rawmakase-models/resolve/ce76dbfa066e6eafb1162048613534ae204346b3",
                "/rawnind-utnet2-bayer/",
                $name
            ),
            fallback: concat!(
                "https://github.com/pch/rawmakase/releases/download/models-rawnind-utnet2-bayer-1/",
                $name
            ),
        }
    };
}

/// RawNIND UtNet2 (Bayer), by Benoit Brummer (UCLouvain), from rawnind_jddc at
/// `e80fb7b14dc440bba38c01f312fd3cc02deb566c`: checkpoint
/// `DenoiserTrainingBayerToProfiledRGB_4ch_2024-02-21-bayer_ms-ssim_mgout_notrans_valeither_-4`
/// (iteration 4350000, SHA-256
/// `c1f7de3de24cb12f4803ee65e11cfb467e9a5b80c3cb7126746ef026106c15e0`), converted to
/// ONNX with darktable-ai `76513a1e379b4265007bae768825b9a581944ff8`
/// `models/rawdenoise-nind/convert.py --in-channels 4 --funit 32 --activation LeakyReLU
/// --opset 20 --height 512 --width 512 --static`; its initializers equal the checkpoint's
/// state dict bit for bit. Taken and redistributed under CC BY 4.0, one of the author's
/// two licences for the weights (docs/dependencies.md).
pub const RAWNIND: DenoiseSpec = DenoiseSpec {
    id: "rawnind-utnet2-bayer",
    version: 1,
    model: denoise_file!(
        "model_bayer.onnx",
        31_057_086,
        "5111f19a66a9377002e2ff4ca87de8e3303085b33e6dc11746e42ce811bf6cc1"
    ),
    notices: [
        denoise_file!(
            "LICENSE",
            18_657,
            "9ba9550ad48438d0836ddab3da480b3b69ffa0aac7b7878b5a0039e7ab429411"
        ),
        denoise_file!("NOTICE", NOTICE_BYTES, NOTICE_SHA256),
        denoise_file!("MODEL_CARD.md", MODEL_CARD_BYTES, MODEL_CARD_SHA256),
    ],
    input: "input",
    output: "output",
    tile: 512,
    overlap: 64,
    providers: &[
        crate::runtime::Provider::Cpu,
        crate::runtime::Provider::CoreML,
    ],
    license: "CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/), one of the author's two \
              licences for the weights (GPL-3.0 or CC BY 4.0); RAWmakase takes CC BY 4.0.",
    attribution: RAWNIND_CREDIT,
};

/// The credit text, once (see [`DenoiseSpec::attribution`]).
const RAWNIND_CREDIT: &str = "AI Denoise uses RawNIND by Benoit Brummer (UCLouvain), model \
UtNet2 (Bayer), licensed under CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/). \
Source: https://github.com/trougnouf/rawnind_jddc. Paper: Benoit Brummer and Christophe De \
Vleeschouwer, Joint Denoising, Demosaicing, and Compression: A Recommended Paradigm for \
Learning-Based Image Workflows (EUVIP 2026); preprint Learning Joint Denoising, Demosaicing, \
and Compression from the Raw Natural Image Noise Dataset, arXiv:2501.08924 \
(https://arxiv.org/abs/2501.08924). Trained on the RawNIND dataset \
(https://doi.org/10.14428/DVN/DEQCIM). Converted by RAWmakase from the original PyTorch \
checkpoint to ONNX; the weights are unchanged. Provided as is, without warranties. RAWmakase \
is not endorsed by the authors.";
const NOTICE_BYTES: u64 = RAWNIND_CREDIT.len() as u64 + 1;
const NOTICE_SHA256: &str = "809b7f8aac9b4bf7134e7543de867b031f194895b280efebe0206e1ddca868e6";
const MODEL_CARD_BYTES: u64 = 3_670;
const MODEL_CARD_SHA256: &str = "3c2d3d55e8bb0d6b4d0b1e5e357ce2cb4aa0e5f0dd6383cd6bf46c44f755ddf6";

/// Every file AI Denoise needs, in a folder of its own.
pub fn denoise_files() -> Vec<ModelFile> {
    std::iter::once(RAWNIND.model)
        .chain(RAWNIND.notices)
        .collect()
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
    }

    #[test]
    fn the_denoiser_is_pinned_and_its_notice_is_the_manifests_credit() {
        use sha2::{Digest, Sha256};
        let files = denoise_files();
        assert_eq!(
            files.iter().map(|f| f.name).collect::<Vec<_>>(),
            ["model_bayer.onnx", "LICENSE", "NOTICE", "MODEL_CARD.md"]
        );
        for file in &files {
            assert_eq!(file.sha256.len(), 64, "{}", file.name);
            assert!(file.url.starts_with(DENOISE_MIRROR), "{}", file.url);
            assert!(
                file.url
                    .ends_with(&format!("/rawnind-utnet2-bayer/{}", file.name))
            );
            assert_eq!(file.fallback, format!("{DENOISE_RELEASE}/{}", file.name));
        }
        let commit = DENOISE_MIRROR.rsplit('/').next().unwrap();
        assert!(commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()));
        // NOTICE is the credit text and a line feed, byte for byte: the one text.
        let notice = format!("{}\n", RAWNIND.attribution);
        assert_eq!(RAWNIND.notices[1].size_bytes, notice.len() as u64);
        let digest: String = Sha256::digest(notice.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(RAWNIND.notices[1].sha256, digest);
        assert!(RAWNIND.attribution.contains("Benoit Brummer"));
        assert!(RAWNIND.attribution.contains("CC BY 4.0"));
        assert!(RAWNIND.attribution.contains("not endorsed"));
        // The model never shares a folder or a file with the selection models.
        assert!(
            !all_files()
                .iter()
                .any(|f| files.iter().any(|d| d.name == f.name))
        );
    }
}
