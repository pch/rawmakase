//! The pinned contract of AI Denoise's model, and the file record every model pack
//! is made of.
//!
//! Everything AI Denoise's tiler, gain fit and installer need to know about its model
//! lives here; nothing else in the crate hard-codes a size, a tensor name or a file.
//! The selection models' contracts are `rawmakase-selection`'s.

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
    }
}
