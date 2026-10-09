use super::*;
use crate::xml::ns::{CRS, RDF};
use crate::{camera_data::Metadata, model::recipe::Recipe};
use anyhow::Result;
use std::path::Path;
fn xml(attrs: &str, body: &str) -> String {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><r:RDF xmlns:r="{RDF}"><r:Description xmlns:c="{CRS}" {attrs}>{body}</r:Description></r:RDF></x:xmpmeta>"#
    )
}
/// Packets and presets of earlier releases named the operators and white balance a
/// recipe kept from before they were measured. One engine renders every edit now:
/// those markers are ignored, whatever they say, and no longer written.
#[test]
fn markers_of_earlier_releases_are_ignored() -> Result<()> {
    let m = Metadata {
        wb: [2., 1., 1.5],
        daylight_wb: [2., 1., 1.5],
        ..Default::default()
    };
    let fresh = Recipe::with_profiles(&m, &[]);
    let text = write::packet(
        &fresh,
        &m,
        &write::Photo {
            settings: true,
            ..Default::default()
        },
    );
    assert!(!text.contains("RAWmakaseOriginal"), "{text}");
    assert!(!text.contains("RAWmakaseMarkers"), "{text}");
    assert!(!text.contains("RAWmakaseWhiteBalanceModel"), "{text}");
    let plain = r#"c:Temperature="5000" c:Tint="10" c:Sharpness="40" c:Clarity2012="20""#;
    let expected = parse(Path::new("plain.xmp"), &xml(plain, ""))?.apply(&fresh, &m, &[], None)?;
    for markers in [
        r#"c:RAWmakaseMarkers="2""#,
        r#"c:RAWmakaseMarkers="3" c:RAWmakaseOriginal="Sharpening,Clarity""#,
        r#"c:RAWmakaseWhiteBalanceModel="Original""#,
        r#"c:RAWmakaseWhiteBalanceModel="Unknown""#,
        r#"c:RAWmakasePreset="1""#,
    ] {
        let attrs = format!("{markers} {plain}");
        let back = parse(Path::new("old.xmp"), &xml(&attrs, ""))?.apply(&fresh, &m, &[], None)?;
        assert_eq!(
            Recipe {
                preset_name: expected.preset_name.clone(),
                preset_settings: expected.preset_settings.clone(),
                ..back
            },
            expected,
            "{markers}"
        );
    }
    Ok(())
}

#[test]
fn lenient_white_balance_failure_preserves_its_basic_edits() -> Result<()> {
    let m = Metadata {
        wb: [2., 1., 1.5],
        daylight_wb: [2., 1., 1.5],
        ..Default::default()
    };
    let base = Recipe {
        temperature: 5100.,
        tint: 7.,
        wb: [1.1, 1., 0.9],
        ..Recipe::default()
    };
    for attrs in [
        r#"c:Temperature="broken" c:Exposure2012="0.7""#,
        r#"c:WhiteBalance="Unsupported" c:Exposure2012="0.7""#,
    ] {
        let preset = parse(Path::new("broken-wb.xmp"), &xml(attrs, ""))?;
        let (back, warnings) = preset.apply_lenient(&base, &m, &[], None)?;
        assert!(!warnings.is_empty());
        assert_eq!(
            (back.temperature, back.tint, back.wb),
            (base.temperature, base.tint, base.wb)
        );
        assert_eq!(back.exposure, 0.7);
    }
    Ok(())
}

/// A photo whose Auto results are fixed, recording what applying settings asked.
struct FakeMeasures {
    image: crate::camera_data::CameraImage,
    white_balance_crops: std::cell::RefCell<Vec<[f32; 4]>>,
    gray_mix_widths: std::cell::RefCell<Vec<u32>>,
}
impl Default for FakeMeasures {
    fn default() -> Self {
        Self {
            image: crate::camera_data::CameraImage {
                width: 1,
                height: 1,
                pixels: vec![[0.2; 3]],
                metadata: Metadata::default(),
                recovered: Default::default(),
                fast: false,
                scale_factor: 1.,
                scale_clipped: 0,
            },
            white_balance_crops: Default::default(),
            gray_mix_widths: Default::default(),
        }
    }
}
impl FakeMeasures {
    const WB: [f32; 3] = [1.5, 1., 1.25];
    const MIX: [f32; 8] = [-0.1, -0.2, -0.2, -0.3, -0.2, 0.1, 0.2, 0.];
}
impl PhotoMeasures for FakeMeasures {
    fn camera_image(&self) -> &crate::camera_data::CameraImage {
        &self.image
    }
    fn auto_white_balance(&self, base: &Recipe) -> Result<Recipe> {
        self.white_balance_crops.borrow_mut().push(base.crop);
        Ok(Recipe {
            wb: Self::WB,
            temperature: 4321.,
            tint: 12.,
            ..base.clone()
        })
    }
    fn auto_gray_mix(&self, _r: &Recipe, m: &Metadata) -> [f32; 8] {
        self.gray_mix_widths.borrow_mut().push(m.width);
        Self::MIX
    }
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
    assert!(r.point_colors.is_empty());
    let hdr = text.replace("c:HDREditMode=\"0\"", "c:HDREditMode=\"1\"");
    assert!(
        parse(Path::new("hdr.xmp"), &hdr)
            .unwrap()
            .apply(&Recipe::default(), &Metadata::default(), &[], None)
            .is_err()
    );
}

/// Swatches as Camera Raw 18.7 stores them: hue in sixths of a turn, then saturation,
/// value, the three shifts, Range and the hue, saturation and luminance ranges.
const SWATCH: &str = "0.425300, 0.729800, 0.603400, 0.500000, -0.300000, 0.200000, 0.500000, 0.000000, 0.333333, 0.666667, 1.000000, 0.000000, 0.549800, 0.909800, 1.000000, 0.072700, 0.622700, 0.982700, 1.000000";

#[test]
fn point_colors_import_and_write_back() {
    let second = "5.950000, 0.550000, 0.450000, -0.400000, 0.000000, 0.000000, 0.200000, 0.000000, 0.200000, 0.600000, 1.000000, 0.000000, 0.370000, 0.730000, 1.000000, 0.000000, 0.522000, 0.882000, 1.000000";
    let invalid = SWATCH.replacen("0.425300", "6.100000", 1);
    let body = format!(
        "<c:PointColors><r:Seq><r:li>{SWATCH}</r:li><r:li>{invalid}</r:li><r:li>{second}</r:li></r:Seq></c:PointColors><c:ColorVariance><r:Seq><r:li>0.400000</r:li><r:li>0</r:li><r:li>-0.250000</r:li></r:Seq></c:ColorVariance>"
    );
    let p = parse(
        Path::new("photo.xmp"),
        &xml("c:Exposure2012=\"0.5\"", &body),
    )
    .unwrap();
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    let r = p
        .apply(&Recipe::default(), &Metadata::default(), &[], None)
        .unwrap();
    // Camera Raw drops the swatch whose hue is out of range; variances stay paired.
    assert_eq!(r.point_colors.len(), 2);
    let [a, b] = [r.point_colors[0], r.point_colors[1]];
    assert_eq!(a.source, [0.4253, 0.7298, 0.6034]);
    assert_eq!(a.shift, [0.5, -0.3, 0.2]);
    assert_eq!(a.variance, 0.4);
    assert_eq!((b.source[0], b.range, b.variance), (5.95, 0.2, -0.25));
    assert_eq!(b.hue_range, [0., 0.2, 0.6, 1.]);
    // Written back as Camera Raw writes them, and read again unchanged.
    let photo = crate::xmp::write::Photo {
        settings: true,
        ..Default::default()
    };
    let packet = crate::xmp::write::packet(&r, &Metadata::default(), &photo);
    assert!(
        packet.contains(&format!("<rdf:li>{SWATCH}</rdf:li>")),
        "{packet}"
    );
    let again = parse(Path::new("again.xmp"), &packet).unwrap();
    let swatches = crate::model::point_color::parse_list(
        &again.settings["PointColors"],
        again.settings.get("ColorVariance").map(String::as_str),
    );
    assert_eq!(swatches.unwrap(), r.point_colors);
    // A photo without swatches writes none.
    let none = crate::xmp::write::packet(&Recipe::default(), &Metadata::default(), &photo);
    assert!(!none.contains("PointColors"));
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
    let m = Metadata {
        width: 32,
        height: 24,
        wb: [2., 1., 1.8],
        daylight_wb: [2., 1., 1.8],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    // Measured on the preset's crop.
    let attrs =
        r#"c:WhiteBalance="Auto" c:CropLeft="0" c:CropTop="0" c:CropRight="0.45" c:CropBottom="1""#;
    let preset = parse(Path::new("preset.xmp"), &xml(attrs, ""))?;
    let photo = FakeMeasures::default();
    let result = preset.apply(&Recipe::default(), &m, &[], Some(&photo))?;
    assert_eq!(result.crop, [0., 0., 0.45, 1.]);
    assert_eq!(
        photo.white_balance_crops.borrow().as_slice(),
        [[0., 0., 0.45, 1.]]
    );
    assert_eq!(
        (result.wb, result.temperature, result.tint),
        (FakeMeasures::WB, 4321., 12.)
    );
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
            r#"<c:Name><r:Alt><r:li xml:lang="x-default">Warm &amp; soft</r:li></r:Alt></c:Name><c:ToneCurvePV2012Red><r:Seq><r:li>1, 12</r:li><r:li>240, 250</r:li></r:Seq></c:ToneCurvePV2012Red><c:ToneCurvePV2012><r:Seq><r:li>0, 0</r:li><r:li>255, 255</r:li></r:Seq></c:ToneCurvePV2012><c:ToneCurvePV2012Green><r:Seq><r:li>0, 0</r:li><r:li>255, 255</r:li></r:Seq></c:ToneCurvePV2012Green><c:ToneCurvePV2012Blue><r:Seq><r:li>0, 0</r:li><r:li>255, 255</r:li></r:Seq></c:ToneCurvePV2012Blue>"#,
        ),
    )?;
    assert_eq!(p.name, "Warm & soft");
    let r = p.apply(&Recipe::default(), &Metadata::default(), &[], None)?;
    assert_eq!(r.curve, ToneCurve::default());
    assert_eq!(r.effects.channels[0].points.len(), 2);
    assert_eq!(r.effects.channels[0].points[0], [1. / 255., 12. / 255.]);
    assert_eq!(r.effects.channels[0].evaluate(0.), 12. / 255.);
    assert_eq!(r.effects.channels[0].evaluate(1.), 250. / 255.);
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
fn profile_amount_applies_to_looks_that_have_one() -> Result<()> {
    use crate::camera_profiles::{CameraProfile, open};
    let m = Metadata {
        make: "Test".into(),
        model: "Camera".into(),
        cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
        ..Default::default()
    };
    let profiles: Vec<_> = [
        open::standard(&m),
        open::color(&m),
        Some(CameraProfile::creative_for_test(&m)),
    ]
    .into_iter()
    .flatten()
    .map(std::sync::Arc::new)
    .collect();
    let base = Recipe::with_profiles(&m, &profiles);
    let preset = |look: &str, amount: &str| {
        parse(
            Path::new("look.xmp"),
            &xml(
                r#"c:Exposure2012="2""#,
                &format!(
                    r#"<c:Look><r:Description c:Name="{look}" c:Amount="{amount}"/></c:Look>"#
                ),
            ),
        )
    };
    // Presets with an Amount are no longer blocked, and apply it.
    let p = preset("Test Creative", "0.5")?;
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    assert!(p.look_available(&m, &profiles));
    let r = p.apply(&base, &m, &profiles, None)?;
    assert_eq!(r.profile.as_ref().unwrap().name, "Test Creative");
    assert_eq!((r.profile_amount, r.exposure), (0.5, 2.));
    // 0% keeps the look, at no strength.
    let r = preset("Test Creative", "0")?.apply(&base, &m, &profiles, None)?;
    assert_eq!(r.profile.as_ref().unwrap().name, "Test Creative");
    assert_eq!(r.profile_amount, 0.);
    // A look without an Amount renders at 100%; a preset that names another profile
    // resets an earlier Amount.
    let earlier = Recipe {
        profile_amount: 0.3,
        ..base
    };
    let r = parse(
        Path::new("look.xmp"),
        &xml(
            &format!(r#"c:CameraProfile="{}""#, open::STANDARD),
            &format!(
                r#"<c:Look><r:Description c:Name="{}" c:Amount="0.5"/></c:Look>"#,
                open::COLOR
            ),
        ),
    )?
    .apply(&earlier, &m, &profiles, None)?;
    assert_eq!(r.profile.as_ref().unwrap().name, open::COLOR);
    assert_eq!(r.profile_amount, 1.);
    // An amount Lightroom cannot store is malformed.
    assert!(preset("Test Creative", "-1").is_err());
    assert!(preset("Test Creative", "2.5").is_err());
    Ok(())
}
#[test]
fn every_lightroom_vignette_style_imports() -> Result<()> {
    use crate::model::effects::VignetteStyle;
    for (code, style) in [
        ("1", VignetteStyle::HighlightPriority),
        ("2", VignetteStyle::ColorPriority),
        ("3", VignetteStyle::PaintOverlay),
    ] {
        let attrs = format!(r#"c:PostCropVignetteAmount="-40" c:PostCropVignetteStyle="{code}""#);
        let r = parse(Path::new("p.xmp"), &xml(&attrs, ""))?.apply(
            &Recipe::default(),
            &Metadata::default(),
            &[],
            None,
        )?;
        assert_eq!(r.effects.vignette_style, style);
    }
    let attrs = r#"c:PostCropVignetteAmount="-40" c:PostCropVignetteStyle="4""#;
    assert!(
        parse(Path::new("p.xmp"), &xml(attrs, ""))?
            .apply(&Recipe::default(), &Metadata::default(), &[], None)
            .is_err()
    );
    Ok(())
}
#[test]
fn refine_saturation_imports_and_out_of_range_values_are_rejected() -> Result<()> {
    let apply = |value: &str| {
        parse(
            Path::new("p.xmp"),
            &xml(&format!("c:CurveRefineSaturation=\"{value}\""), ""),
        )?
        .apply(&Recipe::default(), &Metadata::default(), &[], None)
    };
    assert_eq!(apply("0")?.curve_saturation, 0.);
    assert_eq!(apply("50")?.curve_saturation, 0.5);
    assert_eq!(apply("200")?.curve_saturation, 2.);
    assert!(apply("201").is_err());
    // A preset without it leaves the recipe's own.
    let base = Recipe {
        curve_saturation: 0.3,
        ..Default::default()
    };
    let r = parse(Path::new("p.xmp"), &xml(r#"c:Exposure2012="1""#, ""))?.apply(
        &base,
        &Metadata::default(),
        &[],
        None,
    )?;
    assert_eq!(r.curve_saturation, 0.3);
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
    use crate::model::masks::{MaskOp, MaskShape};
    use crate::model::retouch::{RetouchMode, RetouchShape};
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
    let frame = crate::model::image_frame::ImageFrame::for_metadata(&Metadata {
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
    assert_eq!(u.mode, crate::model::transform::UprightMode::Vertical);
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
    assert_eq!(r.upright.mode, crate::model::transform::UprightMode::Level);
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
    Ok(())
}
#[test]
fn manual_distortion_imports() -> Result<()> {
    let apply = |attrs: &str, r: &Recipe| {
        parse(Path::new("d.xmp"), &xml(attrs, ""))?.apply(r, &Metadata::default(), &[], None)
    };
    let r = apply(r#"c:LensManualDistortionAmount="-35""#, &Recipe::default())?;
    assert!((r.lens_manual_distortion + 0.35).abs() < 1e-6);
    assert_eq!(
        apply(r#"c:Exposure2012="1""#, &r)?.lens_manual_distortion,
        r.lens_manual_distortion
    );
    assert!(apply(r#"c:LensManualDistortionAmount="101""#, &Recipe::default()).is_err());
    Ok(())
}
/// Settings that change how the lens renders leave no Upright correction analysed
/// through the old lens settings; settings that change nothing rendered keep it.
#[test]
fn new_lens_settings_drop_an_upright_analysis_made_through_the_old_ones() -> Result<()> {
    use crate::model::panels::{Panel, PanelState};
    use crate::model::transform::UprightMode;
    let mut base = Recipe::default();
    base.upright.mode = UprightMode::Level;
    base.upright.corrections = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 4];
    let apply = |attrs: &str, r: &Recipe| {
        parse(Path::new("d.xmp"), &xml(attrs, ""))?.apply(r, &Metadata::default(), &[], None)
    };
    let r = apply(r#"c:LensManualDistortionAmount="30""#, &base)?;
    assert!(r.upright.corrections.is_empty());
    assert_eq!(r.upright.mode, UprightMode::Level);
    assert_eq!(apply(r#"c:Exposure2012="1""#, &base)?.upright, base.upright);
    // With the Lens Corrections panel off the amount renders nothing.
    let mut off = base.clone();
    off.panels.set(Panel::LensCorrections, PanelState::Off);
    let r = apply(r#"c:LensManualDistortionAmount="30""#, &off)?;
    assert_eq!(r.upright.corrections, base.upright.corrections);
    // Corrections the settings bring are kept, unless they fail to apply.
    let identity = "1, 0, 0, 0, 1, 0, 0, 0, 1";
    let stored = format!(
        r#"c:LensManualDistortionAmount="30" c:PerspectiveUpright="1" c:UprightTransform_0="{identity}" c:UprightTransform_1="1.1, 0, 0, 0, 1, 0, 0, 0, 1""#
    );
    let r = apply(&stored, &base)?;
    assert_eq!(r.upright.corrections.len(), 2);
    let broken =
        r#"c:LensManualDistortionAmount="30" c:PerspectiveUpright="1" c:UprightTransform_1="oops""#;
    let (r, _) = parse(Path::new("d.xmp"), &xml(broken, ""))?.apply_lenient(
        &base,
        &Metadata::default(),
        &[],
        None,
    )?;
    assert!(r.upright.corrections.is_empty());
    Ok(())
}
/// Lightroom writes `AutoGrayscaleMix` with the mixer it resolved: stored mixer values
/// win, as in Camera Raw. Auto without values (a preset) is estimated from the photo;
/// without one it keeps the current mix.
#[test]
fn auto_grayscale_mix_uses_stored_mixer_or_estimates_it() -> Result<()> {
    let m = Metadata::default();
    let resolved = parse(
        Path::new("photo.xmp"),
        &xml(
            r#"c:ConvertToGrayscale="True" c:AutoGrayscaleMix="True" c:GrayMixerRed="-12" c:GrayMixerBlue="30""#,
            "",
        ),
    )?;
    let r = resolved.apply(&Recipe::default(), &m, &[], None)?;
    assert!(r.effects.monochrome);
    assert_eq!(r.effects.gray_mix[0], -12. * 0.01);
    assert_eq!(r.effects.gray_mix[5], 30. * 0.01);
    // Off, or on for a colour photo, Auto changes nothing.
    for attrs in [
        r#"c:ConvertToGrayscale="True" c:AutoGrayscaleMix="False""#,
        r#"c:ConvertToGrayscale="False" c:AutoGrayscaleMix="True""#,
    ] {
        let p = parse(Path::new("p.xmp"), &xml(attrs, ""))?;
        p.apply(&Recipe::default(), &m, &[], None)?;
    }
    let invalid = parse(
        Path::new("p.xmp"),
        &xml(
            r#"c:ConvertToGrayscale="False" c:AutoGrayscaleMix="Maybe""#,
            "",
        ),
    )?;
    assert!(invalid.apply(&Recipe::default(), &m, &[], None).is_err());
    let mut base = Recipe::default();
    base.effects.gray_mix[2] = 0.4;
    let auto = parse(
        Path::new("auto.xmp"),
        &xml(
            r#"c:ConvertToGrayscale="True" c:AutoGrayscaleMix="True" c:Exposure2012="0.5""#,
            "",
        ),
    )?;
    // Without the photo (listing which presets fit it), the current mix stays, as
    // Auto white balance leaves white balance.
    let r = auto.apply(&base, &m, &[], None)?;
    assert!(r.effects.monochrome);
    assert_eq!(r.exposure, 0.5);
    assert_eq!(r.effects.gray_mix, base.effects.gray_mix);
    // With the photo, Auto alone is estimated from it, as the B&W panel's Auto does.
    let photo_metadata = Metadata {
        width: 16,
        height: 8,
        ..Default::default()
    };
    let photo = FakeMeasures::default();
    let r = auto.apply(&base, &photo_metadata, &[], Some(&photo))?;
    assert!(r.effects.monochrome);
    assert_eq!(r.effects.gray_mix, FakeMeasures::MIX);
    assert_eq!(photo.gray_mix_widths.borrow().as_slice(), [16]);
    // A monochrome profile makes the result black & white as well.
    let m = Metadata {
        cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let mut profile = crate::camera_profiles::CameraProfile::camera_matrix_default(&m)
        .unwrap()
        .with_test_tables();
    profile.enhanced.as_mut().unwrap().monochrome = true;
    let base = Recipe {
        profile: Some(std::sync::Arc::new(profile)),
        ..Default::default()
    };
    let auto = parse(
        Path::new("auto.xmp"),
        &xml(r#"c:AutoGrayscaleMix="True""#, ""),
    )?;
    assert_eq!(auto.apply(&base, &m, &[], None)?.effects.gray_mix, [0.; 8]);
    Ok(())
}
/// Camera Raw reads the Red, Green and Blue point curves only as the full set
/// Lightroom writes, with the master curve; a partial set changes nothing.
#[test]
fn channel_curves_apply_only_as_a_full_set() -> Result<()> {
    let seq = |name: &str, points: &[&str]| {
        let items: String = points.iter().map(|p| format!("<r:li>{p}</r:li>")).collect();
        format!("<c:{name}><r:Seq>{items}</r:Seq></c:{name}>")
    };
    let linear = ["0, 0", "255, 255"];
    let red = seq("ToneCurvePV2012Red", &["0, 0", "128, 150", "255, 255"]);
    let full = [
        seq("ToneCurvePV2012", &linear),
        red.clone(),
        seq("ToneCurvePV2012Green", &linear),
        seq("ToneCurvePV2012Blue", &linear),
    ]
    .concat();
    let r = parse(Path::new("full.xmp"), &xml("", &full))?.apply(
        &Recipe::default(),
        &Metadata::default(),
        &[],
        None,
    )?;
    assert_eq!(r.effects.channels[0].points[1], [128. / 255., 150. / 255.]);
    for partial in [
        red.clone(),
        [seq("ToneCurvePV2012", &linear), red.clone()].concat(),
        [
            red.clone(),
            seq("ToneCurvePV2012Green", &linear),
            seq("ToneCurvePV2012Blue", &linear),
        ]
        .concat(),
    ] {
        let r = parse(Path::new("partial.xmp"), &xml("", &partial))?.apply(
            &Recipe::default(),
            &Metadata::default(),
            &[],
            None,
        )?;
        assert_eq!(r.effects.channels, Recipe::default().effects.channels);
    }
    Ok(())
}
/// Lightroom's Constrain Crop (`CropConstrainToWarp`) imports instead of being refused,
/// apart from `CropConstrainToUnitSquare`, which leaves it off.
#[test]
fn constrain_crop_imports() -> Result<()> {
    let apply = |attrs: &str, r: &Recipe| {
        parse(Path::new("c.xmp"), &xml(attrs, ""))?.apply(r, &Metadata::default(), &[], None)
    };
    let on = apply(r#"c:CropConstrainToWarp="1""#, &Recipe::default())?;
    assert!(on.constrain_crop);
    assert!(apply(r#"c:Exposure2012="1""#, &on)?.constrain_crop);
    assert!(!apply(r#"c:CropConstrainToWarp="0""#, &on)?.constrain_crop);
    assert!(!apply(r#"c:CropConstrainToUnitSquare="1""#, &Recipe::default())?.constrain_crop);
    Ok(())
}
/// Guided Upright's guides as Camera Raw 18.7 serializes them (captured from its own
/// settings for a synthetic DNG): `UprightFourSegmentsCount` and, per guide,
/// `UprightFourSegments_N` = "x1,y1,x2,y2" with nine decimals. They import as guides,
/// write back the same way, and a Guided sidecar with guides but no stored correction
/// opens to be solved on the photo.
#[test]
fn guided_upright_guides_round_trip_as_camera_raw_writes_them() -> Result<()> {
    use crate::model::transform::{UprightGuide, UprightMode};
    let identity = "1.000000000,0.000000000,0.000000000,0.000000000,1.000000000,0.000000000,0.000000000,0.000000000,1.000000000";
    let guided = "1.000000000,0.000000000,0.000000000,0.000000000,1.000000000,0.000000000,0.000000000,0.100000000,1.000000000";
    let attrs = format!(
        r#"xmlns:ps="http://ns.adobe.com/photoshop/1.0/" ps:SidecarForExtension="ARW" c:PerspectiveUpright="5" c:UprightTransformCount="6" c:UprightTransform_0="{identity}" c:UprightTransform_1="{identity}" c:UprightTransform_2="{identity}" c:UprightTransform_3="{identity}" c:UprightTransform_4="{identity}" c:UprightTransform_5="{guided}" c:UprightGuidedDependentDigest="0123456789ABCDEF0123456789ABCDEF" c:UprightFourSegmentsCount="2" c:UprightFourSegments_0="0.300000000,0.100000000,0.250000000,0.900000000" c:UprightFourSegments_1="0.700000000,0.100000000,0.750000000,0.900000000""#
    );
    // Enough of a camera for the written white balance to read back.
    let m = Metadata {
        wb: [2., 1., 1.5],
        daylight_wb: [2., 1., 1.5],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let r = parse(Path::new("photo.xmp"), &xml(&attrs, ""))?.apply(
        &Recipe::default(),
        &m,
        &[],
        None,
    )?;
    let u = &r.upright;
    assert_eq!(u.mode, UprightMode::Guided);
    assert_eq!(
        u.guides,
        [
            UprightGuide {
                a: [0.3, 0.1],
                b: [0.25, 0.9]
            },
            UprightGuide {
                a: [0.7, 0.1],
                b: [0.75, 0.9]
            },
        ]
    );
    assert!(
        u.lightroom
            .keys()
            .all(|k| !k.starts_with("UprightFourSegments"))
    );
    assert_eq!(
        u.lightroom["UprightGuidedDependentDigest"],
        "0123456789ABCDEF0123456789ABCDEF"
    );
    let photo = crate::xmp::write::Photo {
        raw_name: "IMG.ARW".into(),
        settings: true,
        format: "image/jpeg".into(),
        ..Default::default()
    };
    let packet = crate::xmp::write::packet(&r, &m, &photo);
    assert!(
        packet.contains(r#"crs:UprightFourSegmentsCount="2""#),
        "{packet}"
    );
    assert!(packet.contains(
        r#"crs:UprightFourSegments_1="0.700000000,0.100000000,0.750000000,0.900000000""#
    ));
    let back = parse(Path::new("export.xmp"), &packet)?.apply(&Recipe::default(), &m, &[], None)?;
    assert_eq!(back.upright, r.upright);
    // Guides without a stored correction are still reported: renders outside the
    // editor (the command line, Library previews) have no analysis to solve them beside.
    let attrs = r#"xmlns:ps="http://ns.adobe.com/photoshop/1.0/" ps:SidecarForExtension="ARW" c:PerspectiveUpright="5" c:UprightFourSegmentsCount="2" c:UprightFourSegments_0="0.3,0.1,0.25,0.9" c:UprightFourSegments_1="0.7,0.1,0.75,0.9""#;
    assert!(
        parse(Path::new("photo.xmp"), &xml(attrs, ""))?
            .apply(&Recipe::default(), &m, &[], None)
            .is_err()
    );
    // A broken guide is refused, not guessed at.
    let attrs = r#"c:PerspectiveUpright="5" c:UprightFourSegmentsCount="1" c:UprightFourSegments_0="0.3 0.1 0.25 0.9""#;
    assert!(
        parse(Path::new("p.xmp"), &xml(attrs, ""))?
            .apply(&Recipe::default(), &m, &[], None)
            .is_err()
    );
    Ok(())
}
#[test]
fn red_eye_corrections_import_from_camera_raw_and_catalogs() -> Result<()> {
    // As Camera Raw 18.7 writes them (synthetic values).
    let red = "x = 0.520833, y = 0.341797, width = 0.013021, height = 0.019531, alpha = 0.200000, density = 0.750000, strength = 0.080000, redBias = 0.200000, pupilSize = 0.300000, pupilDarkenAmount = 0.700000, adaptivePupilColor = 0, gammaEncodeCorrection = 1, showPetEyeHighlight = 1, highlightX = 0.591000, highlightY = 0.424000";
    let pet = "x = 0.2, y = 0.3, width = 0.01, height = 0.015, alpha = 0.000000, density = 0, strength = 0, redBias = 0, pupilSize = 0.5, pupilDarkenAmount = 0.5, adaptivePupilColor = 1, gammaEncodeCorrection = 1, showPetEyeHighlight = 1, highlightX = 0.591000, highlightY = 0.424000";
    let body =
        format!("<c:RedEyeInfo><r:Seq><r:li>{red}</r:li><r:li>{pet}</r:li></r:Seq></c:RedEyeInfo>");
    let preset = parse(
        Path::new("photo.xmp"),
        &xml(r#"c:EnableRedEye="True""#, &body),
    )?;
    let landscape = Metadata {
        width: 1536,
        height: 1024,
        ..Default::default()
    };
    let (r, warnings) = preset.apply_lenient(&Recipe::default(), &landscape, &[], None)?;
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(r.red_eye.len(), 2);
    // Pet Eye, with Lightroom's default catchlight: 0.5 is the centre and 0 or 1 a
    // semi-axis away.
    use crate::model::red_eye::EyeKind;
    let EyeKind::Pet {
        catchlight: Some(c),
    } = r.red_eye[1].kind
    else {
        panic!("{:?}", r.red_eye[1].kind)
    };
    assert!(
        (c[0] - 0.182).abs() < 1e-5 && (c[1] + 0.152).abs() < 1e-5,
        "{c:?}"
    );
    let eye = &r.red_eye[0];
    let near = |a: f32, b: f32| (a - b).abs() < 1e-5;
    assert!(near(eye.center[0], 0.520833) && near(eye.center[1], 0.341797));
    // Semi-axes of the frame's width and height: a 20-pixel circle.
    assert!(near(eye.radius[0], 20. / 1536.) && near(eye.radius[1], 20. / 1536.));
    assert!(near(eye.correlation, 0.2));
    assert!(near(eye.pupil_size, 0.3) && near(eye.darken, 0.7));
    // A portrait photo: the centre turns, the axes swap and the tilt mirrors.
    let portrait = Metadata {
        flip: 6,
        ..landscape.clone()
    };
    let (r, _) = preset.apply_lenient(&Recipe::default(), &portrait, &[], None)?;
    let eye = &r.red_eye[0];
    assert!(near(eye.center[0], 1. - 0.341797) && near(eye.center[1], 0.520833));
    assert!(near(eye.correlation, -0.2));
    // The catchlight turns with the photo.
    let EyeKind::Pet {
        catchlight: Some(c),
    } = r.red_eye[1].kind
    else {
        panic!()
    };
    assert!(
        (c[0] - 0.152).abs() < 1e-5 && (c[1] - 0.182).abs() < 1e-5,
        "{c:?}"
    );
    // Without Add Catchlight, or with it outside the pupil, there is none.
    for highlight in [
        "showPetEyeHighlight = 0, highlightX = 0.591000, highlightY = 0.424000",
        "showPetEyeHighlight = 1, highlightX = 0.9, highlightY = 0.1",
    ] {
        let pet = format!(
            "x = 0.2, y = 0.3, width = 0.01, height = 0.015, alpha = 0, pupilSize = 0.5, pupilDarkenAmount = 0.5, adaptivePupilColor = 1, gammaEncodeCorrection = 1, {highlight}"
        );
        let body = format!("<c:RedEyeInfo><r:Seq><r:li>{pet}</r:li></r:Seq></c:RedEyeInfo>");
        let preset = parse(
            Path::new("photo.xmp"),
            &xml(r#"c:EnableRedEye="True""#, &body),
        )?;
        let (r, _) = preset.apply_lenient(&Recipe::default(), &landscape, &[], None)?;
        assert_eq!(r.red_eye[0].kind, EyeKind::Pet { catchlight: None });
    }
    // The switch turned off keeps the corrections but renders without them.
    let off = parse(
        Path::new("photo.xmp"),
        &xml(r#"c:EnableRedEye="False""#, &body),
    )?;
    let (r, _) = off.apply_lenient(&Recipe::default(), &landscape, &[], None)?;
    assert_eq!(r.red_eye.len(), 2);
    assert!(r.as_rendered().red_eye.is_empty());
    Ok(())
}
#[test]
fn red_eye_catalog_tables_parse_as_data() -> Result<()> {
    use super::local::Node;
    // Lightroom Classic's catalog form (synthetic values).
    let text = r#"{ { adaptivePupilColor = 0, gammaEncodeCorrection = 1, highlightX = 0.591,
        highlightY = 0.424, pupil = { density = 0.7, ellipse = { alpha = -0.1, centerX = 0.5,
        centerY = 0.4, sizeX = 0.001, sizeY = 0.0015 }, redBias = 0.2, strength = 0.05 },
        pupilDarkenAmount = 0.5, pupilSize = 0.5, showPetEyeHighlight = 1 } }"#;
    let mut local = std::collections::BTreeMap::new();
    local.insert("RedEyeInfo".to_string(), Node::from_lua(text)?);
    let frame = crate::model::image_frame::ImageFrame::for_metadata(&Metadata {
        width: 6000,
        height: 4000,
        ..Default::default()
    });
    let edits = local::convert(&local, frame);
    assert!(edits.skipped.is_empty(), "{:?}", edits.skipped);
    let eyes = edits.red_eye.unwrap();
    assert_eq!(eyes.len(), 1);
    assert_eq!(eyes[0].center, [0.5, 0.4]);
    // 6 pixels each way: a circle.
    assert!((eyes[0].radius[0] - 0.001).abs() < 1e-7 && (eyes[0].radius[1] - 0.001).abs() < 1e-7);
    assert_eq!(eyes[0].correlation, -0.1);
    Ok(())
}
/// A photo black & white only by its profile is written as Lightroom writes it,
/// with its black & white mix.
#[test]
fn black_white_by_profile_writes_its_mix() {
    let m = Metadata {
        cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let mut profile = crate::camera_profiles::CameraProfile::camera_matrix_default(&m)
        .unwrap()
        .with_test_tables();
    profile.enhanced.as_mut().unwrap().monochrome = true;
    let mut r = Recipe {
        profile: Some(std::sync::Arc::new(profile)),
        ..Default::default()
    };
    r.effects.gray_mix[5] = -0.4;
    assert!(!r.effects.monochrome);
    let photo = crate::xmp::write::Photo {
        raw_name: "IMG.ARW".into(),
        settings: true,
        format: "image/jpeg".into(),
        ..Default::default()
    };
    let packet = crate::xmp::write::packet(&r, &m, &photo);
    assert!(
        packet.contains(r#"crs:ConvertToGrayscale="True""#),
        "{packet}"
    );
    assert!(packet.contains(r#"crs:GrayMixerBlue="-40""#), "{packet}");
}
/// Lightroom's lens profile Setup and the profile an edit names import, are kept when
/// it isn't imported, and are written back as read.
#[test]
fn lens_profile_identity_round_trips() -> Result<()> {
    use crate::lens::choice::{LensProfileChoice, LensProfileId, LensProfileSetup, tests};
    let mut m = tests::photo();
    m.wb = [1.; 3];
    m.daylight_wb = [1.; 3];
    m.matrix = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
    let apply = |attrs: &str, r: &Recipe| {
        parse(Path::new("lens.xmp"), &xml(attrs, ""))?.apply(r, &m, &[], None)
    };
    let named = r#"c:LensProfileEnable="1" c:LensProfileSetup="Custom" c:LensProfileName="Adobe (Gone 24mm)" c:LensProfileFilename="Gone (24mm) - RAW.lcp" c:LensProfileDigest="0123456789ABCDEF0123456789ABCDEF" c:LensProfileIsEmbedded="False""#;
    let r = apply(named, &Recipe::default())?;
    let id = LensProfileId {
        name: "Adobe (Gone 24mm)".into(),
        filename: "Gone (24mm) - RAW.lcp".into(),
        digest: "0123456789ABCDEF0123456789ABCDEF".into(),
        embedded: false,
    };
    assert_eq!(
        r.lens_profile_choice,
        LensProfileChoice {
            setup: LensProfileSetup::Custom,
            id: Some(id),
        }
    );
    assert!(r.lens_correction(&m).is_none());
    assert!(
        r.missing_lens_profile(&m)
            .unwrap()
            .contains("Adobe (Gone 24mm)")
    );
    let photo = crate::xmp::write::Photo {
        raw_name: "IMG.RAW".into(),
        settings: true,
        format: "image/jpeg".into(),
        ..Default::default()
    };
    let packet = crate::xmp::write::packet(&r, &m, &photo);
    for key in [
        r#"crs:LensProfileSetup="Custom""#,
        r#"crs:LensProfileName="Adobe (Gone 24mm)""#,
        r#"crs:LensProfileFilename="Gone (24mm) - RAW.lcp""#,
        r#"crs:LensProfileDigest="0123456789ABCDEF0123456789ABCDEF""#,
    ] {
        assert!(packet.contains(key), "{key} {packet}");
    }
    let back = parse(Path::new("export.xmp"), &packet)?.apply(&Recipe::default(), &m, &[], None)?;
    assert_eq!(back.lens_profile_choice, r.lens_profile_choice);
    // Lightroom's names for the setups.
    let auto = apply(r#"c:LensProfileSetup="Auto""#, &r)?;
    assert_eq!(
        auto.lens_profile_choice,
        LensProfileChoice {
            setup: LensProfileSetup::Auto,
            id: None
        }
    );
    let default = apply(r#"c:LensProfileSetup="LensDefaults""#, &r)?;
    assert!(default.lens_profile_choice.is_default());
    // A preset without lens settings leaves the choice alone.
    assert_eq!(
        apply(r#"c:Exposure2012="1""#, &r)?.lens_profile_choice,
        r.lens_profile_choice
    );
    // Under Auto, the profile written is the one rendering.
    let on = Recipe {
        lens_profile: true,
        ..Default::default()
    };
    let packet = crate::xmp::write::packet(&on, &m, &photo);
    assert!(
        packet.contains(r#"crs:LensProfileSetup="LensDefaults""#),
        "{packet}"
    );
    assert!(
        packet.contains(&format!(r#"crs:LensProfileFilename="{}""#, tests::ADOBE)),
        "{packet}"
    );
    // The profile the RAW carries renders as the built-in correction, even with a
    // matching profile imported, is not reported missing and is written back.
    let embedded = apply(
        r#"c:LensProfileEnable="1" c:LensProfileSetup="LensDefaults" c:LensProfileName="Camera Settings" c:LensProfileIsEmbedded="True""#,
        &Recipe::default(),
    )?;
    assert!(embedded.lens_correction(&m).is_none());
    // Without the RAW's own data, that is said; with it, nothing is missing.
    assert!(
        embedded
            .missing_lens_profile(&m)
            .unwrap()
            .contains("Camera Settings")
    );
    let mut with_builtin = m.clone();
    with_builtin.lens = Some(crate::optics::LensCorrection {
        source: "Testcam built-in".into(),
        vignetting: Some(crate::optics::Radial {
            knots: vec![0., 1.],
            values: vec![1., 1.5],
        }),
        ..Default::default()
    });
    let embedded_on = Recipe {
        lens_builtin: true,
        ..embedded.clone()
    };
    assert_eq!(embedded_on.missing_lens_profile(&with_builtin), None);
    assert_eq!(
        embedded_on
            .lens_correction(&with_builtin)
            .map(|l| l.source.as_str()),
        Some("Testcam built-in")
    );
    let packet = crate::xmp::write::packet(&embedded, &m, &photo);
    assert!(
        packet.contains(r#"crs:LensProfileIsEmbedded="True""#),
        "{packet}"
    );
    assert!(
        packet.contains(r#"crs:LensProfileName="Camera Settings""#),
        "{packet}"
    );
    // A file replaced under the same name by another profile is written as rendered,
    // without the old digest.
    let stale = apply(
        &format!(
            r#"c:LensProfileEnable="1" c:LensProfileSetup="Custom" c:LensProfileName="Adobe (Old)" c:LensProfileFilename="{}" c:LensProfileDigest="0123ABCD""#,
            tests::ADOBE
        ),
        &Recipe::default(),
    )?;
    let packet = crate::xmp::write::packet(&stale, &m, &photo);
    assert!(
        packet.contains(r#"crs:LensProfileName="Adobe (Testcam 35mm F2)""#),
        "{packet}"
    );
    assert!(!packet.contains("0123ABCD"), "{packet}");
    // Lightroom's other boolean spelling.
    let one = apply(
        r#"c:LensProfileSetup="LensDefaults" c:LensProfileName="Camera Settings" c:LensProfileIsEmbedded="1""#,
        &Recipe::default(),
    )?;
    assert!(one.lens_profile_choice.id.unwrap().embedded);
    // Under Auto, a named profile that isn't imported stays named.
    let gone = apply(
        r#"c:LensProfileEnable="1" c:LensProfileSetup="Auto" c:LensProfileName="Adobe (Gone 24mm)" c:LensProfileFilename="Gone (24mm) - RAW.lcp""#,
        &Recipe::default(),
    )?;
    let packet = crate::xmp::write::packet(&gone, &m, &photo);
    assert!(
        packet.contains(r#"crs:LensProfileFilename="Gone (24mm) - RAW.lcp""#),
        "{packet}"
    );
    Ok(())
}
#[test]
fn lightroom_manual_vignetting_imports() -> Result<()> {
    let apply = |attrs: &str| {
        parse(Path::new("p.xmp"), &xml(attrs, ""))?.apply(
            &Recipe::default(),
            &Metadata::default(),
            &[],
            None,
        )
    };
    let r = apply(r#"c:VignetteAmount="-50" c:VignetteMidpoint="20""#)?;
    assert_eq!(r.effects.lens_vignette, -0.5);
    assert!((r.effects.lens_vignette_midpoint - 0.2).abs() < 1e-6);
    Ok(())
}

/// A packet records the crop it is given as rendered, else the recipe's own.
#[test]
fn packets_record_the_rendered_crop() {
    let r = Recipe {
        crop: [0.1, 0.2, 0.9, 0.8],
        ..Default::default()
    };
    let mut photo = crate::xmp::write::Photo {
        settings: true,
        ..Default::default()
    };
    let packet = crate::xmp::write::packet(&r, &Metadata::default(), &photo);
    assert!(packet.contains(r#"crs:CropLeft="0.100000""#), "{packet}");
    photo.crop = Some([0.15, 0.25, 0.85, 0.75]);
    let packet = crate::xmp::write::packet(&r, &Metadata::default(), &photo);
    assert!(packet.contains(r#"crs:CropLeft="0.150000""#), "{packet}");
    assert!(packet.contains(r#"crs:CropBottom="0.750000""#), "{packet}");
}
