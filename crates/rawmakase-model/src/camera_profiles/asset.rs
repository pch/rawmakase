//! A camera profile as a catalog stores it once and names it by content
//! ([`crate::storage::recipe_assets`]).
//!
//! A profile's asset is its *normalized* value, the one every reader holds in
//! memory: a saved profile is read, then [`CameraProfile::loaded`] gives an
//! Embedded profile today's tone curve. A fresh save, the catalog's migration of an
//! inline profile and a read of a stored one all go through this one sequence, so
//! an Embedded profile saved with an old tone curve is named as a fresh save of the
//! same photo would name it.
use super::CameraProfile;

/// The profile as every reader holds it: an Embedded profile takes today's tone
/// curve, any other is unchanged. Normalizing a normalized profile changes nothing.
pub fn normalized(p: CameraProfile) -> CameraProfile {
    p.loaded()
}

/// Whether `p` is already what [`normalized`] makes of it.
pub fn is_normalized(p: &CameraProfile) -> bool {
    !p.is_embedded() || p.tone == super::default_tone()
}

/// The bytes a profile is stored and named by: the JSON of its normalized value, as
/// the derived `Serialize` writes it. f32 values are written as their shortest text
/// that reads back to the same f32, and a profile serializes no hash map, so the
/// same value always gives the same bytes.
pub fn canonical_bytes(p: &CameraProfile) -> Vec<u8> {
    let bytes = if is_normalized(p) {
        serde_json::to_vec(p)
    } else {
        serde_json::to_vec(&normalized(p.clone()))
    };
    bytes.expect("a camera profile serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_data::{Metadata, Source};
    use crate::storage::recipe_assets::{AssetKind, asset_id};

    /// An original synthetic DCP: a camera, a name and a forward matrix.
    fn dcp() -> Vec<u8> {
        let text = |s: &str| format!("{s}\0").into_bytes();
        let entries = [
            (50708u16, 2u16, text("Test Camera")),
            (50936, 2, text("Test Standard")),
            (
                50964,
                10,
                (0..9)
                    .flat_map(|i| [700 + 37 * i, 1000])
                    .flat_map(i32::to_le_bytes)
                    .collect(),
            ),
        ];
        let mut at = 8 + 2 + entries.len() * 12 + 4;
        let mut bytes = b"IIRC".to_vec();
        bytes.extend(8u32.to_le_bytes());
        bytes.extend((entries.len() as u16).to_le_bytes());
        for (tag, kind, data) in &entries {
            bytes.extend(tag.to_le_bytes());
            bytes.extend(kind.to_le_bytes());
            bytes.extend((if *kind == 10 { 9 } else { data.len() as u32 }).to_le_bytes());
            bytes.extend((at as u32).to_le_bytes());
            at += data.len();
        }
        bytes.extend(0u32.to_le_bytes());
        for (_, _, data) in entries {
            bytes.extend(data);
        }
        bytes
    }
    fn camera() -> Metadata {
        Metadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [[0.9, -0.2, -0.05], [-0.4, 1.2, 0.15], [-0.05, 0.1, 0.6]],
            wb: [2., 1., 1.5],
            daylight_wb: [2., 1., 1.5],
            ..Default::default()
        }
    }
    /// One profile of every kind a recipe can hold.
    fn every_kind() -> Vec<(&'static str, CameraProfile)> {
        let m = camera();
        let matrix = CameraProfile::camera_matrix_default(&m).unwrap();
        let rendered = Metadata {
            source: Source::Rendered,
            ..m.clone()
        };
        let mut old_embedded = serde_json::to_value(CameraProfile::embedded(&rendered)).unwrap();
        old_embedded["tone"] = serde_json::json!([[0., 0.], [1., 1.]]);
        let cube = "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n0.9 0.95 1\n";
        let fitted =
            super::super::fitted::with_look(&matrix, &super::super::fitted::FittedLook::identity())
                .unwrap();
        vec![
            ("dcp", super::super::from_bytes(&dcp()).unwrap()),
            ("embedded", CameraProfile::embedded(&rendered)),
            (
                "embedded with an old tone curve",
                serde_json::from_value(old_embedded).unwrap(),
            ),
            ("camera_matrix_default", matrix.clone()),
            (
                "builtin",
                (*super::super::builtin(&Metadata {
                    embedded_dcp: Some(dcp().into()),
                    ..m
                })
                .unwrap())
                .clone(),
            ),
            ("hdr", matrix.clone().as_hdr_for_test()),
            ("fitted", fitted),
            (
                "enhanced",
                matrix.clone().with_test_look(
                    3,
                    |v| 1. + v / 4.,
                    &[[0., 0.], [0.5, 0.6], [1., 1.]],
                ),
            ),
            ("srgb look", matrix.clone().with_test_srgb_look()),
            ("cube", matrix.with_cube_for_test(cube).unwrap()),
        ]
    }

    /// Reading a profile's canonical bytes gives what reading it inline gives, and
    /// they name it stably: `normalize(parse(canonical(x))) == normalize(parse(x))`.
    #[test]
    fn canonical_bytes_read_back_as_the_profile_for_every_kind() {
        for (kind, p) in every_kind() {
            let inline = serde_json::to_vec(&p).unwrap();
            let from_inline = normalized(serde_json::from_slice(&inline).unwrap());
            let canonical = canonical_bytes(&p);
            let from_canonical: CameraProfile = serde_json::from_slice(&canonical).unwrap();
            assert_eq!(normalized(from_canonical.clone()), from_inline, "{kind}");
            assert_eq!(
                from_canonical, from_inline,
                "{kind}: canonical is normalized"
            );
            assert_eq!(
                canonical_bytes(&from_canonical),
                canonical,
                "{kind}: stable"
            );
            assert_eq!(canonical_bytes(&from_inline), canonical, "{kind}");
        }
    }

    /// An Embedded profile saved with an old tone curve is named as a fresh save of
    /// the same photo's profile is.
    #[test]
    fn an_old_embedded_profile_has_a_fresh_profiles_id() {
        let kinds = every_kind();
        let of = |name: &str| &kinds.iter().find(|(k, _)| *k == name).unwrap().1;
        let old = of("embedded with an old tone curve");
        assert_ne!(old, of("embedded"));
        assert_eq!(
            asset_id(AssetKind::Profile, &canonical_bytes(old)),
            asset_id(AssetKind::Profile, &canonical_bytes(of("embedded")))
        );
        let ids: std::collections::BTreeSet<_> = kinds
            .iter()
            .map(|(_, p)| asset_id(AssetKind::Profile, &canonical_bytes(p)))
            .collect();
        // DCP and builtin are the same profile, and so are the two Embedded ones.
        assert_eq!(ids.len(), kinds.len() - 2);
    }
}
