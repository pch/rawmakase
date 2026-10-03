use super::ns::{CRS, RDF};
use super::*;
use crate::{develop::Recipe, raw::Metadata};
use anyhow::Result;
use std::path::Path;
fn xml(attrs: &str, body: &str) -> String {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><r:RDF xmlns:r="{RDF}"><r:Description xmlns:c="{CRS}" {attrs}>{body}</r:Description></r:RDF></x:xmpmeta>"#
    )
}
#[test]
fn legacy_split_toning_restores_full_overlap_but_modern_presets_preserve_it() -> Result<()> {
    let mut base = Recipe::default();
    base.grading[1] = [0.3, 0.4, 0.2];
    base.effects.global_grade = [0.5, 0.6, -0.1];
    base.effects.blending = 0.2;
    let old = parse(
        Path::new("old.xmp"),
        &xml(
            r#"c:SplitToningShadowHue="240" c:SplitToningShadowSaturation="50""#,
            "",
        ),
    )?;
    let r = old.apply(&base, &Metadata::default(), &[], None)?;
    assert!(r.reference_color);
    assert_eq!(r.grading[0], [2. / 3., 0.5, 0.]);
    assert_eq!(r.effects.blending, 1.);
    assert_eq!(r.grading[1], [0.; 3]);
    assert_eq!(r.effects.global_grade, [0.; 3]);
    let modern = parse(
        Path::new("modern.xmp"),
        &xml(
            r#"c:SplitToningShadowHue="240" c:ColorGradeBlending="75""#,
            "",
        ),
    )?;
    let r = modern.apply(&base, &Metadata::default(), &[], None)?;
    assert_eq!(r.effects.blending, 0.75);
    assert_eq!(r.grading[1], base.grading[1]);
    assert_eq!(r.effects.global_grade, base.effects.global_grade);
    Ok(())
}

#[test]
fn photo_sidecar_empty_point_colors_and_preset_provenance() {
    let sentinel = vec!["-1.000000"; 19].join(", ");
    let body = format!(
        "<c:PointColors><r:Seq><r:li>{sentinel}</r:li></r:Seq></c:PointColors><c:ColorVariance><r:Seq><r:li>-50</r:li></r:Seq></c:ColorVariance><c:Preset><r:Description c:Amount=\"100\"><c:Parameters><r:Description c:Exposure2012=\"7\"/></c:Parameters></r:Description></c:Preset>"
    );
    let attrs = "xmlns:ps=\"http://ns.adobe.com/photoshop/1.0/\" ps:SidecarForExtension=\"RAF\" c:Exposure2012=\"0.5\" c:HDREditMode=\"0\" c:CurveRefineSaturation=\"100\"";
    let text = xml(attrs, &body);
    let p = parse(Path::new("photo.xmp"), &text).unwrap();
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    let r = p
        .apply(&Recipe::default(), &Metadata::default(), &[], None)
        .unwrap();
    assert_eq!(r.exposure, 0.5);
    let active = text.replacen("-1.000000", "0.25", 1);
    assert!(
        !parse(Path::new("active.xmp"), &active)
            .unwrap()
            .blockers
            .is_empty()
    );
    let hdr = text.replace("c:HDREditMode=\"0\"", "c:HDREditMode=\"1\"");
    assert!(
        parse(Path::new("hdr.xmp"), &hdr)
            .unwrap()
            .apply(&Recipe::default(), &Metadata::default(), &[], None)
            .is_err()
    );
}

#[test]
fn resolved_photo_white_balance_differs_from_as_shot_preset() -> Result<()> {
    let m = Metadata {
        wb: [2., 1., 1.8],
        daylight_wb: [2., 1., 1.8],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let attrs = r#"c:WhiteBalance="As Shot" c:Temperature="6500" c:Tint="25""#;
    let preset = parse(Path::new("preset.xmp"), &xml(attrs, ""))?;
    let result = preset.apply(&Recipe::default(), &m, &[], None)?;
    assert_eq!(result.wb, [1.; 3]);
    let sidecar = parse(
        Path::new("photo.xmp"),
        &xml(
            &format!(
                r#"{attrs} xmlns:ps="http://ns.adobe.com/photoshop/1.0/" ps:SidecarForExtension="RAF""#
            ),
            "",
        ),
    )?;
    let result = sidecar.apply(&Recipe::default(), &m, &[], None)?;
    assert_eq!(result.temperature, 6500.);
    assert_eq!(result.tint, 25.);
    assert_ne!(result.wb, [1.; 3]);
    Ok(())
}
#[test]
fn named_white_balance_presets_apply_their_values() -> Result<()> {
    let m = Metadata {
        wb: [2., 1., 1.8],
        daylight_wb: [2., 1., 1.8],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    // A preset naming Lightroom's Daylight without values gets the menu's Daylight.
    let preset = parse(
        Path::new("preset.xmp"),
        &xml(r#"c:WhiteBalance="Daylight""#, ""),
    )?;
    // Applied over Auto, it no longer reads as Auto.
    let auto = Recipe {
        auto_white_balance: Some([5500., 10.]),
        ..Default::default()
    };
    let r = preset.apply(&auto, &m, &[], None)?;
    assert_eq!((r.temperature, r.tint), (5500., 10.));
    assert_eq!(r.auto_white_balance, None);
    // Stored values win, and one missing value falls back to the name's.
    let attrs = r#"c:WhiteBalance="Flash" c:Temperature="5300""#;
    let preset = parse(Path::new("preset.xmp"), &xml(attrs, ""))?;
    let r = preset.apply(&Recipe::default(), &m, &[], None)?;
    assert_eq!((r.temperature, r.tint), (5300., 0.));
    assert!(
        parse(
            Path::new("preset.xmp"),
            &xml(r#"c:WhiteBalance="Moonlight""#, "")
        )?
        .apply(&Recipe::default(), &m, &[], None)
        .is_err()
    );
    Ok(())
}
#[test]
fn auto_white_balance_presets_use_the_wb_menus_auto() -> Result<()> {
    let (width, height) = (32u32, 24u32);
    let m = Metadata {
        width,
        height,
        wb: [2., 1., 1.8],
        daylight_wb: [2., 1., 1.8],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let im = crate::raw::CameraImage {
        recovered: Default::default(),
        width,
        height,
        // A warm left half and a cool right half.
        pixels: (0..width * height)
            .map(|i| {
                let x = i % width;
                let v = 0.05 + 0.4 * x as f32 / width as f32;
                if x < width / 2 {
                    [v * 1.3, v, v * 0.7]
                } else {
                    [v * 0.7, v, v * 1.3]
                }
            })
            .collect(),
        metadata: m.clone(),
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    };
    // Measured on the preset's crop: the warm half.
    let attrs =
        r#"c:WhiteBalance="Auto" c:CropLeft="0" c:CropTop="0" c:CropRight="0.45" c:CropBottom="1""#;
    let preset = parse(Path::new("preset.xmp"), &xml(attrs, ""))?;
    let result = preset.apply(&Recipe::default(), &m, &[], Some(&im))?;
    assert_eq!(result.crop, [0., 0., 0.45, 1.]);
    let cropped = Recipe {
        crop: result.crop,
        ..Default::default()
    };
    let auto = crate::develop::auto_white_balance(&im, &cropped)?;
    let whole = crate::develop::auto_white_balance(&im, &Recipe::default())?;
    assert_ne!(auto.wb, whole.wb);
    assert_eq!(
        (result.wb, result.temperature, result.tint),
        (auto.wb, auto.temperature, auto.tint)
    );
    assert_eq!(result.auto_white_balance, auto.auto_white_balance);
    Ok(())
}
#[test]
fn partial_preset_preserves_omitted_settings_and_zero_resets() -> Result<()> {
    let p = parse(
        Path::new("toolkit.xmp"),
        &xml(
            r#"c:Contrast2012="0" c:Saturation="-20" c:ConvertToGrayscale="False""#,
            "",
        ),
    )?;
    let mut r = Recipe {
        exposure: 1.7,
        contrast: 0.5,
        temperature: 4700.,
        ..Default::default()
    };
    r.effects.monochrome = true;
    let out = p.apply(&r, &Metadata::default(), &[], None)?;
    assert_eq!(out.exposure, 1.7);
    assert_eq!(out.temperature, 4700.);
    assert_eq!(out.contrast, 0.);
    assert!((out.saturation + 0.2).abs() < 1e-6);
    assert!(!out.effects.monochrome);
    Ok(())
}
#[test]
fn names_entities_and_rgb_curves_parse_with_namespace_aliases() -> Result<()> {
    let p = parse(
        Path::new("preset.xmp"),
        &xml(
            "",
            r#"<c:Name><r:Alt><r:li xml:lang="x-default">Warm &amp; soft</r:li></r:Alt></c:Name><c:ToneCurvePV2012Red><r:Seq><r:li>1, 12</r:li><r:li>240, 250</r:li></r:Seq></c:ToneCurvePV2012Red>"#,
        ),
    )?;
    assert_eq!(p.name, "Warm & soft");
    let r = p.apply(&Recipe::default(), &Metadata::default(), &[], None)?;
    assert_eq!(r.curve, ToneCurve::default());
    assert_eq!(r.effects.channels[0].points.len(), 2);
    assert_eq!(r.effects.channels[0].points[0], [1. / 255., 12. / 255.]);
    assert_eq!(r.effects.channels[0].evaluate(0.), 12. / 255.);
    assert_eq!(r.effects.channels[0].evaluate(1.), 250. / 255.);
    assert!(r.reference_curves);
    Ok(())
}
#[test]
fn child_scalar_values_and_multiple_descriptions() -> Result<()> {
    let text = format!(
        r#"<r:RDF xmlns:r="{RDF}" xmlns:c="{CRS}" xmlns:d="urn:other"><r:Description><d:Exposure2012>99</d:Exposure2012></r:Description><r:Description><c:Exposure2012>+0.5</c:Exposure2012><c:Clarity2012>-20</c:Clarity2012></r:Description></r:RDF>"#
    );
    let p = parse(Path::new("p.xmp"), &text)?;
    let r = p.apply(&Recipe::default(), &Metadata::default(), &[], None)?;
    assert_eq!(r.exposure, 0.5);
    assert!((r.effects.clarity + 0.2).abs() < 1e-6);
    Ok(())
}
#[test]
fn missing_profiles_unknown_settings_and_looks_never_partially_apply() -> Result<()> {
    let base = Recipe {
        exposure: 1.,
        ..Default::default()
    };
    for attrs in [
        r#"c:Exposure2012="2" c:CameraProfile="Missing""#,
        r#"c:Exposure2012="2" c:ImaginaryControl="0""#,
    ] {
        let p = parse(Path::new("p.xmp"), &xml(attrs, ""))?;
        assert!(p.apply(&base, &Metadata::default(), &[], None).is_err());
        assert_eq!(base.exposure, 1.);
    }
    let p = parse(
        Path::new("look.xmp"),
        &xml(
            r#"c:Exposure2012="2""#,
            r#"<c:Look><r:Description c:Name="Adobe Color" c:Exposure2012="7"/></c:Look>"#,
        ),
    )?;
    assert_eq!(p.settings["Exposure2012"], "2");
    assert!(p.apply(&base, &Metadata::default(), &[], None).is_err());
    Ok(())
}
#[test]
fn profile_amount_blocks_strict_presets_and_lenient_application_reports_it() -> Result<()> {
    let base = Recipe {
        exposure: 1.,
        ..Default::default()
    };
    let p = parse(
        Path::new("look.xmp"),
        &xml(
            r#"c:Exposure2012="2""#,
            r#"<c:Look><r:Description c:Name="Adobe Color" c:Amount="0.5"/></c:Look>"#,
        ),
    )?;
    assert!(
        p.blockers.iter().any(|b| b.contains("Profile Amount 50%")),
        "{:?}",
        p.blockers
    );
    assert!(p.apply(&base, &Metadata::default(), &[], None).is_err());
    // An amount Lightroom cannot store is malformed.
    assert!(
        parse(
            Path::new("look.xmp"),
            &xml(
                r#"c:Exposure2012="2""#,
                r#"<c:Look><r:Description c:Name="Adobe Color" c:Amount="-1"/></c:Look>"#,
            ),
        )
        .is_err()
    );
    // At 0% the look does nothing, so nothing blocks.
    let p = parse(
        Path::new("look.xmp"),
        &xml(
            r#"c:Exposure2012="2""#,
            r#"<c:Look><r:Description c:Name="Adobe Color" c:Amount="0"/></c:Look>"#,
        ),
    )?;
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    assert!(p.look.is_empty());
    assert_eq!(
        p.apply(&base, &Metadata::default(), &[], None)?.exposure,
        2.
    );
    Ok(())
}
#[test]
fn malformed_numbers_rejected() -> Result<()> {
    for value in ["NaN", "inf", "oops", "900"] {
        let p = parse(
            Path::new("p.xmp"),
            &xml(&format!("c:Exposure2012=\"{value}\""), ""),
        )?;
        assert!(
            p.apply(&Recipe::default(), &Metadata::default(), &[], None)
                .is_err()
        );
    }
    Ok(())
}
#[test]
fn specific_duplicate_group_repair_is_reported() -> Result<()> {
    let text = format!(
        r#"<rdf:RDF xmlns:rdf="{RDF}" xmlns:crs="{CRS}"><rdf:Description crs:Contrast2012="1">
<crs:Group><rdf:Alt><rdf:li>Film</rdf:li></rdf:Alt>
</crs:Group>
</crs:Group>
</rdf:Description></rdf:RDF>"#
    );
    let p = parse(Path::new("p.xmp"), &text)?;
    assert_eq!(p.group, "Film");
    assert_eq!(p.notes.len(), 1);
    Ok(())
}
#[test]
fn lenient_apply_keeps_supported_settings_and_reports_the_rest() -> Result<()> {
    let base = Recipe {
        exposure: 1.,
        ..Default::default()
    };
    let p = parse(
        Path::new("p.xmp"),
        &xml(r#"c:Exposure2012="2" c:CameraProfile="Missing""#, ""),
    )?;
    let (recipe, skipped) = p.apply_lenient(&base, &Metadata::default(), &[], None)?;
    assert_eq!(recipe.exposure, 2.);
    assert_eq!(recipe.profile, base.profile);
    assert!(skipped.iter().any(|s| s.contains("Missing camera profile")));
    Ok(())
}
#[test]
fn empty_flags_are_unset_and_curves_still_apply() -> Result<()> {
    let p = parse(
        Path::new("p.xmp"),
        &xml(
            r#"c:ConvertToGrayscale="" c:ToneCurveName2012="Custom""#,
            r#"<c:ToneCurvePV2012><r:Seq><r:li>0, 50</r:li><r:li>255, 255</r:li></r:Seq></c:ToneCurvePV2012>"#,
        ),
    )?;
    let r = p.apply(&Recipe::default(), &Metadata::default(), &[], None)?;
    assert!(!r.effects.monochrome);
    assert_eq!(r.curve.points[0], [0., 50. / 255.]);
    Ok(())
}
/// Lightroom's spots and masks convert with the conventions measured on Camera Raw:
/// unrotated positions, long-edge sizes, absolute sources (a brush's from its first
/// dab); AI masks are reported, not imported.
#[test]
fn lightroom_spots_and_masks_convert_to_image_space() -> Result<()> {
    use crate::develop::masks::{MaskOp, MaskShape};
    use crate::develop::retouch::{RetouchMode, RetouchShape};
    let dabs =
        "<r:li>d 0.200000 0.700000</r:li><r:li>r 0.030000</r:li><r:li>d 0.300000 0.700000</r:li>";
    let body = format!(
        r#"<c:RetouchAreas><r:Seq>
            <r:li><r:Description c:SpotType="clone" c:SourceX="0.7" c:OffsetY="0.6" c:Opacity="0.8" c:Feather="0.5">
              <c:Masks><r:Seq><r:li c:What="Mask/Ellipse" c:X="0.3" c:Y="0.3" c:SizeX="0.05" c:SizeY="0.05"/></r:Seq></c:Masks>
            </r:Description></r:li>
            <r:li><r:Description c:SpotType="heal" c:SourceX="0.6" c:OffsetY="0.3">
              <c:Masks><r:Seq><r:li><r:Description c:What="Mask/Paint" c:Radius="0.03" c:Flow="1" c:CenterWeight="1">
                <c:Dabs><r:Seq>{dabs}</r:Seq></c:Dabs></r:Description></r:li></r:Seq></c:Masks>
            </r:Description></r:li>
          </r:Seq></c:RetouchAreas>
          <c:MaskGroupBasedCorrections><r:Seq>
            <r:li><r:Description c:What="Correction" c:CorrectionName="Sky" c:CorrectionAmount="0.8" c:LocalExposure2012="-0.5" c:LocalDehaze="0.3" c:LocalToningHue="180" c:LocalToningSaturation="0.4">
              <c:CorrectionMasks><r:Seq>
                <r:li c:What="Mask/Gradient" c:MaskBlendMode="0" c:ZeroX="0.5" c:ZeroY="0.5" c:FullX="0.5" c:FullY="0.1"/>
                <r:li><r:Description c:What="Mask/Paint" c:MaskBlendMode="1" c:MaskValue="0" c:Radius="0.02" c:Flow="0.5" c:CenterWeight="0.25">
                  <c:Dabs><r:Seq>{dabs}</r:Seq></c:Dabs></r:Description></r:li>
              </r:Seq></c:CorrectionMasks>
            </r:Description></r:li>
            <r:li><r:Description c:What="Correction" c:LocalShadows2012="0.4">
              <c:CorrectionMasks><r:Seq>
                <r:li c:What="Mask/CircularGradient" c:Top="0.2" c:Left="0.1" c:Bottom="0.5" c:Right="0.4" c:Angle="-10" c:Feather="40" c:Flipped="false" c:Midpoint="50" c:Roundness="0"/>
              </r:Seq></c:CorrectionMasks>
            </r:Description></r:li>
            <r:li><r:Description c:What="Correction" c:CorrectionName="Subject" c:LocalExposure2012="1">
              <c:CorrectionMasks><r:Seq><r:li c:What="Mask/Image" c:MaskSubType="1"/></r:Seq></c:CorrectionMasks>
            </r:Description></r:li>
          </r:Seq></c:MaskGroupBasedCorrections>"#
    );
    let preset = parse(
        Path::new("photo.xmp"),
        &xml(r#"c:Exposure2012="0.2""#, &body),
    )?;
    assert!(preset.blockers.is_empty(), "{:?}", preset.blockers);
    let landscape = Metadata {
        width: 6000,
        height: 4000,
        ..Default::default()
    };
    // Strict application reports the AI mask; lenient application imports the rest.
    assert!(
        preset
            .apply(&Recipe::default(), &landscape, &[], None)
            .is_err()
    );
    let (r, skipped) = preset.apply_lenient(&Recipe::default(), &landscape, &[], None)?;
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert!(skipped[0].contains("Select Subject"));
    assert_eq!(r.exposure, 0.2);
    let near = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5;
    let spot = &r.retouch[0];
    assert_eq!(
        (spot.mode, spot.opacity, spot.feather),
        (RetouchMode::Clone, 0.8, 0.5)
    );
    assert!(
        matches!(spot.shape, RetouchShape::Spot { center, radius } if near(center, [0.3, 0.3]) && radius == 0.05)
    );
    assert!(near(spot.offset, [0.4, 0.3]));
    let brush = &r.retouch[1];
    assert!(
        matches!(&brush.shape, RetouchShape::Brush { points, radius } if points.len() == 2 && *radius == 0.03)
    );
    // The source is where the first dab copies from.
    assert!(near(brush.offset, [0.4, -0.4]));
    assert_eq!(r.masks.len(), 2);
    let sky = &r.masks[0];
    assert_eq!((sky.name.as_str(), sky.amount), ("Sky", 0.8));
    assert_eq!(sky.adjust.exposure, -0.5);
    assert_eq!(sky.adjust.color, [0.5, 0.4]);
    assert!(
        matches!(sky.components[0].shape, MaskShape::Linear { from, to } if near(from, [0.5, 0.1]) && near(to, [0.5, 0.5]))
    );
    let erase = &sky.components[1];
    assert_eq!(erase.op, MaskOp::Subtract);
    assert!(
        matches!(&erase.shape, MaskShape::Brush { strokes } if strokes[0].erase && strokes[0].flow == 0.5 && strokes[0].feather == 0.75)
    );
    let radial = &r.masks[1].components[0];
    // Not flipped: the effect is outside the ellipse, Lightroom's default.
    assert!(radial.invert);
    assert!(
        matches!(radial.shape, MaskShape::Radial { center, radii, angle, feather }
        if near(center, [0.25, 0.35]) && (radii[0] - 0.15).abs() < 1e-5 && (radii[1] - 0.1).abs() < 1e-5
            && angle == 10. && feather == 0.4)
    );
    // A portrait photo (camera turned 90° clockwise): positions turn, sizes do not.
    let portrait = Metadata {
        flip: 6,
        ..landscape
    };
    let (r, _) = preset.apply_lenient(&Recipe::default(), &portrait, &[], None)?;
    assert!(
        matches!(r.retouch[0].shape, RetouchShape::Spot { center, radius } if near(center, [0.7, 0.3]) && radius == 0.05)
    );
    assert!(near(r.retouch[0].offset, [-0.3, 0.4]));
    assert!(
        matches!(r.masks[1].components[0].shape, MaskShape::Radial { angle, .. } if angle == 100.)
    );
    Ok(())
}
#[test]
fn lightroom_catalog_tables_parse_as_data() -> Result<()> {
    use super::local::Node;
    let text = r#"{ { Feather = 0,
        Masks = { { CenterWeight = 0.5, Dabs = { "d 0.1 0.2", "r 0.02", "M 0.15 0.2" },
            Flow = 1, MaskValue = 1, Radius = 0.03, What = "Mask/Paint" } },
        OffsetY = 0.3, Opacity = 1, SourceX = 0.4, SpotType = "heal" } }"#;
    let node = Node::from_lua(text)?;
    let mut local = std::collections::BTreeMap::new();
    local.insert("RetouchAreas".to_string(), node);
    let frame = crate::develop::ImageFrame::for_metadata(&Metadata {
        width: 300,
        height: 200,
        ..Default::default()
    });
    let edits = local::convert(&local, frame);
    assert!(edits.skipped.is_empty(), "{:?}", edits.skipped);
    let ops = edits.retouch.unwrap();
    assert_eq!(ops.len(), 1);
    assert!((ops[0].offset[0] - 0.3).abs() < 1e-6 && (ops[0].offset[1] - 0.1).abs() < 1e-6);
    assert!(Node::from_lua("{ a = os.exit() }").is_err());
    assert!(Node::from_lua("{ \"unterminated }").is_err());
    Ok(())
}
#[test]
fn upright_imports_lightroom_stored_corrections() -> Result<()> {
    let identity = "1.000000000,0.000000000,0.000000000,0.000000000,1.000000000,0.000000000,0.000000000,0.000000000,1.000000000";
    let vertical = "1.324242917,-0.010468300,-0.000000000,-0.979818960,1.404760585,0.799977181,-2.019842538,-0.000002219,3.019842538";
    let attrs = format!(
        r#"c:Exposure2012="0.5" c:PerspectiveUpright="4" c:UprightVersion="151388160" c:UprightCenterMode="0" c:UprightFocalLength35mm="34.9225" c:UprightTransformCount="6" c:UprightTransform_0="{identity}" c:UprightTransform_1="{identity}" c:UprightTransform_2="{identity}" c:UprightTransform_3="{identity}" c:UprightTransform_4="{vertical}" c:UprightTransform_5="{identity}""#
    );
    let r = parse(Path::new("upright.xmp"), &xml(&attrs, ""))?.apply(
        &Recipe::default(),
        &Metadata::default(),
        &[],
        None,
    )?;
    let u = &r.upright;
    assert_eq!(u.mode, crate::develop::UprightMode::Vertical);
    assert_eq!(u.corrections.len(), 6);
    assert!((u.corrections[4][6] + 2.019_842_6).abs() < 1e-6);
    assert_eq!(u.lightroom["UprightFocalLength35mm"], "34.9225");
    assert!(!u.lightroom.contains_key("UprightTransformCount"));
    // A preset names only the mode: it applies, and the app analyses each photo.
    let preset = parse(
        Path::new("preset.xmp"),
        &xml(r#"c:Exposure2012="0.5" c:PerspectiveUpright="3""#, ""),
    )?;
    let r = preset.apply(&Recipe::default(), &Metadata::default(), &[], None)?;
    assert_eq!(r.exposure, 0.5);
    assert_eq!(r.upright.mode, crate::develop::UprightMode::Level);
    assert!(r.upright.corrections.is_empty());
    // A photo's own settings without Lightroom's correction are reported.
    let sidecar = parse(
        Path::new("photo.xmp"),
        &xml(
            r#"xmlns:ps="http://ns.adobe.com/photoshop/1.0/" ps:SidecarForExtension="ARW" c:Exposure2012="0.5" c:PerspectiveUpright="3""#,
            "",
        ),
    )?;
    let (r, warnings) =
        sidecar.apply_lenient(&Recipe::default(), &Metadata::default(), &[], None)?;
    assert_eq!(r.exposure, 0.5);
    assert!(r.upright.is_default());
    assert!(warnings.iter().any(|w| w.contains("PerspectiveUpright")));
    Ok(())
}
#[test]
fn upright_modes_without_a_stored_correction_stay_unanalysed() {
    // Only Vertical's correction, not Off's to Level's: nothing is filled in for them,
    // so the preset's Vertical is analysed like any mode-only preset.
    let preset = parse(
        Path::new("partial.xmp"),
        &xml(
            r#"c:PerspectiveUpright="4" c:UprightTransform_4="1,0,0,0,1,0,0,0.1,1""#,
            "",
        ),
    )
    .unwrap();
    let r = preset
        .apply(&Recipe::default(), &Metadata::default(), &[], None)
        .unwrap();
    assert!(r.upright.corrections.is_empty());
}
#[test]
fn remove_chromatic_aberration_imports_and_presets_leave_it_when_omitted() -> Result<()> {
    let apply = |attrs: &str, r: &Recipe| {
        parse(Path::new("ca.xmp"), &xml(attrs, ""))?.apply(r, &Metadata::default(), &[], None)
    };
    let on = apply(r#"c:AutoLateralCA="1""#, &Recipe::default())?;
    assert!(on.lens_ca);
    assert!(apply(r#"c:Exposure2012="0.5""#, &on)?.lens_ca);
    assert!(!apply(r#"c:AutoLateralCA="0""#, &on)?.lens_ca);
    // Recipes before engine 4 render without lens corrections, so it is refused.
    let old = Recipe {
        engine: 3,
        ..Default::default()
    };
    assert!(apply(r#"c:AutoLateralCA="1""#, &old).is_err());
    assert!(!apply(r#"c:AutoLateralCA="0""#, &old)?.lens_ca);
    Ok(())
}
