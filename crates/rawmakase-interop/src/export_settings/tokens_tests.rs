use super::super::{CameraNames, ExportSettings, NameFields, Naming};
use super::*;

/// The templates Lightroom Classic ships (`naming_token_presets`), in its form.
const LIGHTROOM: [&str; 10] = [
    "{{custom_token}}-{{image_filename_number_suffix}}",
    "{{custom_token}}-{{naming_sequenceNumber_1Digit}}",
    "{{custom_token}} ({{naming_sequenceNumber_1Digit}} of {{naming_sequenceTotal_1Digit}})",
    "{{custom_token}}",
    "{{date_YYYYMMDD}}-{{image_name}}",
    "{{image_name}}-{{naming_sequenceNumber_1Digit}}",
    "{{image_name}}",
    "{{shoot_custom_token}}-{{naming_sequenceNumber_3Digits}}",
    "{{shoot_custom_token}}-{{image_filename_number_suffix}}",
    "{{shoot_custom_token}}-{{naming_sequenceNumber_3Digits}}",
];

/// The Sony a7R VI ARW measured in Lightroom (PLAN Revision 4), as the
/// catalog and its file give it.
fn a7r_vi() -> NameFields {
    NameFields {
        captured: Some("2026-04-24T18:43:24.000".into()),
        info: crate::metadata::PhotoInfo {
            lens: Some("FE 100-400mm F4.5 GM OSS".into()),
            focal: Some(158.),
            aperture: Some(5.),
            exposure: Some(1. / 500.),
            iso: Some(100.),
            dimensions: Some((9984, 6656)),
            camera: Some("SONY ILCE-7RM6".into()),
        },
        camera: CameraNames::Read {
            make: Some("Sony".into()),
            model: Some("ILCE-7RM6".into()),
        },
        ..NameFields::default()
    }
}
const SOURCE: &str = "/Users/someone/Desktop/a7r vi/6489169691.arw";

fn name(tokens: &str, fields: &NameFields, text: &str) -> (String, Vec<String>) {
    let at = NameContext {
        index: 0,
        total: 1,
        fields,
    };
    TokenTemplate::parse(tokens).stem(Path::new(SOURCE), &at, text, 1)
}

#[test]
fn lightrooms_templates_read_and_write_back_exactly() {
    for text in LIGHTROOM {
        let template = TokenTemplate::parse(text);
        assert_eq!(template.to_lightroom(), text);
        assert!(
            template
                .parts()
                .iter()
                .all(|p| matches!(p, Part::Text(_) | Part::Token { token: Some(_), .. })),
            "{text}"
        );
        assert_eq!(
            TokenTemplate::from_display(&template.to_display()).as_ref(),
            Ok(&template),
            "{text}"
        );
    }
    // Lightroom's presets are RAWmakase's, token for token.
    for naming in Naming::ALL {
        assert_eq!(Naming::from_tokens(naming.tokens()), Some(naming));
    }
}

#[test]
fn any_text_reads_and_writes_back_exactly() {
    for text in [
        "",
        "{",
        "}",
        "{{",
        "}}",
        "{{}}",
        "a{{b",
        "{{a}}}",
        "{{{a}}",
        "{{future_token}}-x",
        "{{future}field}}",
        "{{a{{b}}c}}",
        "naïve 日本 {{image_name}} \\ {x}",
        "{{com.adobe.model}}{{com.adobe.model}}",
        "{{naming_sequenceNumber_3Digit}}",
    ] {
        let template = TokenTemplate::parse(text);
        assert_eq!(template.to_lightroom(), text, "{text}");
        assert_eq!(
            TokenTemplate::from_display(&template.to_display()).as_ref(),
            Ok(&template),
            "{text} as {}",
            template.to_display()
        );
    }
}

#[test]
fn the_editor_shows_tokens_by_their_labels_and_others_by_their_ids() {
    let template = TokenTemplate::parse(
        "{{date_YYYY}}-{{com.adobe.model}} {{future}field}} {x}\\ {{naming_sequenceNumber_3Digit}}",
    );
    assert_eq!(
        template.to_display(),
        "{Date (YYYY)}-{Model} {?future\\}field} \\{x\\}\\\\ {?naming_sequenceNumber_3Digit}"
    );
    // Lightroom reads numbering ids by pattern; one it doesn't spell as its
    // menu does is still a sequence number, shown by its id.
    assert_eq!(
        token("naming_sequenceNumber_3Digit"),
        Some(Token::Sequence(3))
    );
    assert_eq!(
        token("naming_importNumber_2Digit"),
        Some(Token::Unsupported("Import #"))
    );
    assert_eq!(token("naming_sequenceNumber_Digit"), None);
    // Labels read in any case, and each names one token.
    assert_eq!(
        TokenTemplate::from_display("{date (yyyy)}").unwrap(),
        TokenTemplate::parse("{{date_YYYY}}")
    );
    for def in TOKENS {
        assert_eq!(
            by_label(def.label).map(|d| d.id),
            Some(def.id),
            "{}",
            def.label
        );
        assert!(!def.label.contains(['{', '}', '\\']), "{}", def.label);
    }
}

#[test]
fn the_editors_mistakes_are_said_where_they_are() {
    let errors = |text: &str| -> Vec<(std::ops::Range<usize>, String)> {
        TokenTemplate::from_display(text)
            .unwrap_err()
            .into_iter()
            .map(|e| (e.at, e.message))
            .collect()
    };
    assert_eq!(errors("ab{Filename")[0].0, 2..11);
    assert_eq!(errors("ab}")[0].0, 2..3);
    let unknown = errors("x {Nope} y");
    assert_eq!(unknown[0].0, 2..8);
    assert!(unknown[0].1.contains("Nope"), "{unknown:?}");
    assert_eq!(errors("{?}")[0].0, 0..3);
    // Text Lightroom's form would read as a token can't be kept.
    assert!(TokenTemplate::from_display("\\{\\{a\\}\\}").is_err());
    assert!(TokenTemplate::from_display("\\{{Filename}").is_err());
    // A brace alone, escaped, is text.
    assert_eq!(
        TokenTemplate::from_display("\\{a\\}")
            .unwrap()
            .to_lightroom(),
        "{a}"
    );
    // Every stretch of the text is shown as what it is.
    let text = "a{Filename}{?x}{Flash}{Nope}";
    let spans: Vec<Span> = TokenTemplate::spans(text)
        .into_iter()
        .map(|s| s.1)
        .collect();
    assert_eq!(
        spans,
        [
            Span::Text,
            Span::Token,
            Span::Inert,
            Span::Inert,
            Span::Error
        ]
    );
}

/// Round trips of many generated templates: ids of any characters, Unicode
/// text, braces and backslashes, as Lightroom's form and the editor's.
#[test]
fn generated_templates_round_trip() {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    const PIECES: [&str; 12] = ["a", "日", "é", " ", "-", "{", "}", "\\", ".", "/", ":", "?"];
    for _ in 0..3000 {
        let mut parts = Vec::new();
        for _ in 0..next(6) {
            match next(3) {
                0 => {
                    let def = &TOKENS[next(TOKENS.len())];
                    parts.push(Part::token(def.id));
                }
                1 => {
                    let id: String = (0..1 + next(4))
                        .map(|_| PIECES[next(PIECES.len())])
                        .collect();
                    parts.push(Part::token(&id));
                }
                _ => {
                    let text: String = (0..1 + next(5))
                        .map(|_| PIECES[next(PIECES.len())])
                        .collect();
                    parts.push(Part::Text(text));
                }
            }
        }
        let template = TokenTemplate::from_parts(parts);
        let stored = template.to_lightroom();
        assert_eq!(TokenTemplate::parse(&stored).to_lightroom(), stored);
        // What Lightroom's form keeps reads back from the editor's; what it
        // can't keep (text that would read as a token) the editor refuses.
        let storable = TokenTemplate::parse(&stored) == template;
        let shown = TokenTemplate::from_display(&template.to_display());
        if storable {
            assert_eq!(shown.as_ref(), Ok(&template), "{}", template.to_display());
        } else {
            assert!(shown.is_err(), "{}", template.to_display());
        }
        // What was stored always shows and reads back.
        let read = TokenTemplate::parse(&stored);
        assert_eq!(
            TokenTemplate::from_display(&read.to_display()).as_ref(),
            Ok(&read)
        );
    }
}

/// Lightroom's Original filename: the name the file had when it entered the
/// catalog, without its extension; the current name when the catalog
/// doesn't know it (a photo added before it kept them).
#[test]
fn original_filename_is_the_name_at_import() {
    let renamed = NameFields {
        original_name: Some("DSCF0001.RAF".into()),
        ..NameFields::default()
    };
    let named = |tokens: &str, f: &NameFields| name(tokens, f, "").0;
    assert_eq!(named("{{image_originalName}}", &renamed), "DSCF0001");
    assert_eq!(
        named("{{image_name}}-{{image_originalName}}", &renamed),
        "6489169691-DSCF0001"
    );
    assert_eq!(
        named("{{image_originalName}}", &NameFields::default()),
        "6489169691"
    );
    let template = TokenTemplate::parse("{{image_originalName}}");
    assert!(template.names_each_photo());
    assert!(!template.needs().any());
    assert_eq!(template.to_display(), "{Original filename}");
    assert_eq!(template.to_lightroom(), "{{image_originalName}}");
}

/// The tokens filled from the file alone, the creators and the keywords,
/// as Lightroom filled them for a synthetic JPEG.
#[test]
fn file_facts_artist_and_keywords_fill_as_lightroom_filled_them() {
    let f = NameFields {
        facts: crate::export_settings::FileFacts {
            software: Some("ProbeSoft 1.2".into()),
            focal_35mm: Some(35),
            headline: Some("Probe Headline".into()),
            ..Default::default()
        },
        creator: Some("A One, B Two".into()),
        artist: Some("A One".into()),
        keywords: "Apple, Zebra".into(),
        ..NameFields::default()
    };
    let one = |tokens: &str| name(tokens, &f, "").0;
    assert_eq!(one("{{com.adobe.software}}"), "ProbeSoft 1.2");
    assert_eq!(one("{{com.adobe.focalLength35mm}}"), "35 mm");
    assert_eq!(one("{{com.adobe.artist}}"), "A One");
    assert_eq!(one("{{com.adobe.creator}}"), "A One, B Two");
    assert_eq!(one("{{com.adobe.keywords}}"), "Apple, Zebra");
    assert_eq!(one("{{com.adobe.headline}}"), "Probe Headline");
    // The file's facts are read for them; keywords are the catalog's.
    let needs = |tokens: &str| TokenTemplate::parse(tokens).needs();
    assert!(needs("{{com.adobe.city}}").facts);
    assert!(needs("{{com.adobe.artist}}").descriptive);
    assert!(!needs("{{com.adobe.keywords}}").any());
    // Without them: a note for what the file must say, nothing for an
    // unset field.
    let (stem, notes) = name(
        "x{{com.adobe.software}}{{com.adobe.city}}",
        &NameFields::default(),
        "",
    );
    assert_eq!(stem, "x");
    assert_eq!(notes, ["no Software"]);
}

/// Lightroom writes an Exposure's aperture with the region's decimal
/// separator, its shutter speed and focal length always with a dot ("0.8 sec
/// at f - 5,0", "23.5 mm" in Poland), and literal text as typed.
#[test]
fn only_the_aperture_has_the_regions_decimal_separator() {
    let f = NameFields {
        info: crate::metadata::PhotoInfo {
            exposure: Some(0.8),
            aperture: Some(5.),
            focal: Some(23.5),
            ..Default::default()
        },
        decimal: crate::export_settings::Decimal(','),
        ..NameFields::default()
    };
    let one = |tokens: &str| name(tokens, &f, "").0;
    assert_eq!(one("{{com.adobe.exposure}}"), "0.8 sec at f - 5,0");
    assert_eq!(one("{{com.adobe.focalLength}}"), "23.5 mm");
    assert_eq!(one("v1.2 {{naming_sequenceNumber_2Digits}}"), "v1.2 01");
}

#[test]
fn tokens_fill_as_lightroom_filled_them_for_the_a7r_vi() {
    let f = a7r_vi();
    let one = |tokens: &str| name(tokens, &f, "").0;
    assert_eq!(
        one(BUILTIN[0].tokens),
        "2026-04-24 18.43.24 ILCE-7RM6",
        "Piotr's example"
    );
    assert_eq!(one("{{com.adobe.exposure}}"), "1-500 sec at f - 5.0");
    assert_eq!(one("{{com.adobe.ISOSpeedRating}}"), "ISO 100");
    assert_eq!(one("{{com.adobe.focalLength}}"), "158 mm");
    assert_eq!(one("{{com.adobe.make}}"), "Sony");
    assert_eq!(one("{{com.adobe.lens}}"), "FE 100-400mm F4.5 GM OSS");
    assert_eq!(one("{{com.adobe.imageFileDimensions}}"), "9984 x 6656");
    assert_eq!(one("{{date_Month}}"), "April");
    assert_eq!(one("{{date_Mon}}"), "Apr");
    assert_eq!(one("{{date_Julian}}"), "114");
    assert_eq!(one("{{date_LocalEncoding}}"), "April 24, 2026");
    assert_eq!(one("{{date_YYMMDD}}"), "260424");
    assert_eq!(one("{{image_filename_number_suffix}}"), "6489169691");
    assert_eq!(one("{{image_folder}}"), "a7r vi");
    assert_eq!(
        one("{{naming_sequenceNumber_3Digits}}-{{naming_sequenceTotal_3Digits}}"),
        "001-001"
    );
    // Unset: empty, and nothing to say about it.
    let (unset, notes) = name(
        "x{{com.adobe.rating.string}}{{com.adobe.colorLabels.string}}{{com.adobe.title}}{{com.adobe.caption}}",
        &f,
        "",
    );
    assert_eq!((unset.as_str(), notes.len()), ("x", 0));
    // A custom text and a typed literal: their slashes and colons are dashes.
    assert_eq!(
        name("{{custom_token}}", &f, "a/b:c*?\"<>|d").0,
        "a-b-c------d"
    );
    assert_eq!(one("x/y:z"), "x-y-z");
    assert_eq!(one("{{custom_token}}"), "untitled");
}

#[test]
fn rawmakases_own_templates_name_the_fixture() {
    let f = a7r_vi();
    let named: Vec<String> = BUILTIN
        .iter()
        .map(|b| {
            let template = TokenTemplate::parse(b.tokens);
            assert_eq!(template.to_lightroom(), b.tokens);
            assert_eq!(
                TokenTemplate::from_display(&template.to_display()).as_ref(),
                Ok(&template)
            );
            assert!(
                template.parts().iter().all(|p| match p {
                    Part::Text(_) => true,
                    Part::Token { token, .. } => {
                        !matches!(token, None | Some(Token::Unsupported(_)))
                    }
                }),
                "{}",
                b.name
            );
            assert_eq!(builtin(b.tokens).map(|b| b.name), Some(b.name));
            assert_eq!(Naming::from_tokens(b.tokens), None, "not a preset");
            name(b.tokens, &f, "").0
        })
        .collect();
    assert_eq!(
        named,
        [
            "2026-04-24 18.43.24 ILCE-7RM6",
            "2026-04-24 18.43.24 6489169691",
            "2026-04-24-001",
        ]
    );
    let names: Vec<&str> = BUILTIN.iter().map(|b| b.name).collect();
    for naming in Naming::ALL {
        assert!(!names.contains(&naming.label()));
    }
}

#[test]
fn what_a_photo_lacks_is_empty_and_said() {
    let none = NameFields::default();
    let (stem, notes) = name(
        "{{date_YYYY}}{{date_Hour}} {{com.adobe.model}}{{com.adobe.lens}}{{image_name}}",
        &none,
        "",
    );
    assert_eq!(stem, "6489169691");
    assert_eq!(
        notes,
        [
            "no capture date",
            "no capture time",
            "the camera's make and model were not read",
            "no Lens",
        ]
    );
    // Read, but the file has no model.
    let read = NameFields {
        camera: CameraNames::Read {
            make: Some("Sony".into()),
            model: None,
        },
        captured: Some("2026-04-24".into()),
        ..NameFields::default()
    };
    let (stem, notes) = name("{{date_YYYY}} {{date_Hour}} {{com.adobe.model}}", &read, "");
    assert_eq!(stem, "2026");
    assert_eq!(notes, ["no capture time", "no Model"]);
    // Nothing at all: the file's own name, said.
    let (stem, notes) = name("{{com.adobe.title}} ..", &none, "");
    assert_eq!(stem, "6489169691");
    assert_eq!(notes.len(), 1);
    // Unknown and unsupported tokens are said once.
    let (_, notes) = name(
        "{{future}}{{future}}{{com.adobe.flash}}{{naming_importNumber_3Digits}}",
        &none,
        "",
    );
    assert_eq!(
        notes,
        [
            "unknown file name token {{future}}",
            "not supported yet: Flash",
            "not supported yet: Import # (001)",
            "the template gave no name: named after the file",
        ]
    );
}

#[test]
fn capture_dates_are_read_strictly() {
    let date = |captured: &str| {
        let f = NameFields::captured(Some(captured.into()));
        name("{{date_YYYYMMDD}} {{date_Hour}}{{date_Julian}}", &f, "")
    };
    assert_eq!(date("2024-02-29 23:59:59").0, "20240229 23060");
    assert_eq!(date("2024:12:31T00:00:00.5").0, "20241231 00366");
    assert_eq!(date("2023:12:31 01:02:03").0, "20231231 01365");
    for bad in [
        "2023-02-29 10:00:00",
        "1900-02-29",
        "0000-01-01",
        "2026-13-01",
        "2026-04-31",
        "2026-00-10",
        "2026/04/24",
        "26-04-24",
        "",
    ] {
        let (stem, notes) = date(bad);
        assert!(
            notes.contains(&"no capture date".to_string()),
            "{bad}: {stem}"
        );
    }
    // A date without a (real) time names the day, not the hour.
    for timeless in ["2026-04-24", "2026-04-24 24:00:00", "2026-04-24 10:60:00"] {
        let (stem, notes) = date(timeless);
        assert_eq!(stem, "20260424 114", "{timeless}");
        assert_eq!(notes, ["no capture time"]);
    }
}

#[test]
fn no_template_names_another_folder_or_a_device() {
    let f = NameFields::default();
    for (tokens, text) in [
        ("/abs/{{image_name}}", ""),
        ("..\\{{image_name}}", ""),
        ("../../x", ""),
        ("{{custom_token}}", "../../etc/passwd"),
        ("a\u{0}b\nc", ""),
        ("{{image_name}}. . ", ""),
        ("  .hidden", ""),
    ] {
        let stem = name(tokens, &f, text).0;
        assert!(!stem.contains(['/', '\\', ':', '\0', '\n']), "{stem}");
        assert!(
            !stem.starts_with('.') && !stem.ends_with(['.', ' ']),
            "{stem}"
        );
        assert!(!stem.is_empty());
    }
    // A template that gives nothing names the file after itself, as safely.
    let at = NameContext {
        index: 0,
        total: 1,
        fields: &f,
    };
    let unset = TokenTemplate::parse("{{com.adobe.title}}");
    for (source, stem) in [
        ("/photos/CON.ARW", "CON_"),
        ("/photos/a:b.ARW", "a-b"),
        ("/photos/..ARW", "untitled"),
    ] {
        assert_eq!(
            unset.stem(Path::new(source), &at, "", 1).0,
            stem,
            "{source}"
        );
    }
    // A device name made of two parts is still a device name.
    assert_eq!(name("{{custom_token}}n", &f, "co").0, "con_");
    assert_eq!(name("{{custom_token}}1.txt", &f, "LPT").0, "LPT1_.txt");
}

#[test]
fn what_a_template_needs_and_uses() {
    let needs = |tokens: &str| TokenTemplate::parse(tokens).needs();
    assert_eq!(needs("{{image_name}}-{{custom_token}}"), Needs::default());
    assert!(needs("{{date_Hour}}").captured);
    assert!(needs("{{com.adobe.make}}").camera);
    assert!(needs("{{com.adobe.lens}}").info && !needs("{{com.adobe.lens}}").camera);
    // A title the catalog lacks is the file's.
    assert!(needs("{{com.adobe.title}}").descriptive && !needs("{{com.adobe.title}}").camera);
    assert!(!needs("{{com.adobe.rating.string}}").any());
    let t = TokenTemplate::parse("{{custom_token}} {{naming_sequenceTotal_1Digit}}");
    assert!(t.uses_text() && !t.uses_sequence() && !t.names_each_photo());
    assert!(TokenTemplate::parse("{{naming_sequenceNumber_2Digits}}").names_each_photo());
    assert!(TokenTemplate::parse("{{image_name}}").names_each_photo());
}

#[test]
fn a_template_numbers_from_start_number() {
    let f = NameFields::default();
    let at = NameContext {
        index: 4,
        total: 120,
        fields: &f,
    };
    let template = TokenTemplate::parse(
        "{{naming_sequenceNumber_3Digits}} of {{naming_sequenceTotal_1Digit}}",
    );
    assert_eq!(
        template.stem(Path::new("/a/b.dng"), &at, "", 7).0,
        "011 of 120"
    );
}

#[test]
fn settings_name_files_by_their_custom_template() {
    let source = Path::new(SOURCE);
    let fields = a7r_vi();
    let at = NameContext {
        index: 0,
        total: 1,
        fields: &fields,
    };
    let mut s = ExportSettings::default();
    s.choose_tokens(BUILTIN[0].tokens);
    assert_eq!(s.naming, Some(Naming::Filename));
    assert_eq!(s.custom_tokens(), Some(BUILTIN[0].tokens));
    assert_eq!(
        s.file_name_for(source, &at).0,
        "2026-04-24 18.43.24 ILCE-7RM6.jpg"
    );
    assert!(s.name_template().needs().camera);
    // A preset's own tokens choose the preset.
    s.choose_tokens(Naming::DateFilename.tokens());
    assert_eq!(
        (s.naming(), s.custom_tokens()),
        (Naming::DateFilename, None)
    );
    // Made over the preset chosen then, and dropped by choosing a preset.
    s.choose_tokens("{{com.adobe.model}}");
    let saved = serde_json::to_value(&s).unwrap();
    assert_eq!(
        saved["name_template"],
        serde_json::json!({"tokens": "{{com.adobe.model}}", "over": "DateFilename"})
    );
    assert_eq!(s.file_name_for(source, &at).0, "ILCE-7RM6.jpg");
    s.choose_naming(Naming::Filename);
    assert_eq!(s.custom_tokens(), None);
    assert_eq!(s.file_name_for(source, &at).0, "6489169691.jpg");
}

#[test]
fn older_and_later_releases_keep_their_choices() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("export.json");
    let saved = |file: &Path| -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::from_str(&std::fs::read_to_string(file)?)?)
    };
    // Saved here with a custom template.
    let mut s = ExportSettings::default();
    s.choose_tokens(BUILTIN[1].tokens);
    s.save_to(&file)?;
    let read = ExportSettings::load_from(&file).unwrap();
    assert_eq!(read.custom_tokens(), Some(BUILTIN[1].tokens));

    // An earlier release reads the file: its struct has no `name_template`,
    // which it keeps among the fields it doesn't know, and `naming` it reads.
    #[derive(serde::Serialize, serde::Deserialize)]
    struct Older {
        naming: Option<Naming>,
        quality: u8,
        #[serde(flatten)]
        later: serde_json::Map<String, serde_json::Value>,
    }
    let mut older: Older = serde_json::from_value(saved(&file)?)?;
    assert_eq!(older.naming, Some(Naming::Filename));
    std::fs::write(&file, serde_json::to_string(&older)?)?;
    assert_eq!(
        ExportSettings::load_from(&file).unwrap().custom_tokens(),
        Some(BUILTIN[1].tokens)
    );
    // It chooses another preset: that names the files here too.
    older.naming = Some(Naming::FilenameSequence);
    std::fs::write(&file, serde_json::to_string(&older)?)?;
    let read = ExportSettings::load_from(&file).unwrap();
    assert_eq!(
        (read.naming(), read.custom_tokens()),
        (Naming::FilenameSequence, None)
    );
    // The template stays in the file until a naming is chosen here.
    read.save_to(&file)?;
    assert_eq!(saved(&file)?["name_template"]["tokens"], BUILTIN[1].tokens);

    // A later release's Rename To and fields inside the template are saved
    // back, and the other settings still load.
    std::fs::write(
        &file,
        r#"{"naming":"FutureNaming","quality":70,"name_template":{"tokens":"{{image_name}}","over":"FutureNaming","case":"title"}}"#,
    )?;
    let mut read = ExportSettings::load_from(&file).unwrap();
    assert_eq!(read.quality, 70);
    assert_eq!(
        (read.naming(), read.custom_tokens()),
        (Naming::Filename, None)
    );
    read.quality = 60;
    read.save_to(&file)?;
    let written = saved(&file)?;
    assert_eq!(written["naming"], "FutureNaming");
    assert_eq!(written["name_template"]["case"], "title");
    assert_eq!(written["quality"], 60);
    // Choosing a template here replaces the later release's choice and keeps
    // what it added inside the template.
    read.choose_tokens("{{com.adobe.model}}");
    read.save_to(&file)?;
    let written = saved(&file)?;
    assert_eq!(written["naming"], "Filename");
    assert_eq!(
        written["name_template"],
        serde_json::json!({"tokens": "{{com.adobe.model}}", "over": "Filename", "case": "title"})
    );
    let read = ExportSettings::load_from(&file).unwrap();
    assert_eq!(read.custom_tokens(), Some("{{com.adobe.model}}"));
    // A template a later release wrote in another shape loads as none.
    std::fs::write(&file, r#"{"quality":50,"name_template":[1,2]}"#)?;
    let read = ExportSettings::load_from(&file).unwrap();
    assert_eq!((read.quality, read.custom_tokens()), (50, None));
    read.save_to(&file)?;
    assert_eq!(saved(&file)?["name_template"], serde_json::json!([1, 2]));
    Ok(())
}
