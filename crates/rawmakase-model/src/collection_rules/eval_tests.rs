//! Evaluation tables: Lightroom Classic 15.6's measured results (issue
//! #998's LRC-FACTS, pass 3) as synthetic entries, and the documented choices
//! for what wasn't measured.
use super::*;
use crate::collection_rules::{EditFacts, Group};
use crate::metadata::PhotoInfo;

/// Tuesday 17 November 2026, weeks starting on Sunday: the day and locale of
/// the measurements.
fn clock() -> Clock {
    Clock {
        today: Date::from_ymd(2026, 11, 17),
        week_start: 0,
    }
}

fn one_rule(criteria: &str, operation: &str, value: Option<Value>) -> Group {
    Group::new(
        Combine::Intersect,
        vec![Item::Rule(Rule::new(criteria, operation, value))],
    )
}
fn text(s: &str) -> Option<Value> {
    Some(Value::Str(s.into()))
}
fn num(n: f64) -> Option<Value> {
    Some(Value::Number(n))
}

/// Which of `entries` (by index) the rules match.
fn matching(group: &Group, entries: &[Entry<'_>]) -> Vec<usize> {
    let matcher = group.compile(&clock()).unwrap();
    (0..entries.len())
        .filter(|i| matcher.matches(&entries[*i]))
        .collect()
}

/// The eight titled photos of the measurement and one without a title.
const TITLES: [&str; 9] = [
    "Fred",
    "reddish",
    "red-car",
    "RED",
    "blue red sky",
    "red fox",
    "fred blue",
    "blue",
    "",
];

fn titled() -> Vec<Entry<'static>> {
    TITLES
        .iter()
        .map(|title| Entry {
            title,
            ..Entry::default()
        })
        .collect()
}

fn titles_matching(operation: &str, value: &str) -> Vec<&'static str> {
    matching(&one_rule("title", operation, text(value)), &titled())
        .into_iter()
        .map(|i| TITLES[i])
        .collect()
}

#[test]
fn title_rules_match_as_measured_in_lightroom() {
    assert_eq!(
        titles_matching("any", "red"),
        [
            "Fred",
            "reddish",
            "red-car",
            "RED",
            "blue red sky",
            "red fox",
            "fred blue"
        ]
    );
    assert_eq!(
        titles_matching("words", "red"),
        ["RED", "blue red sky", "red fox"]
    );
    assert_eq!(
        titles_matching("all", "red blue"),
        ["blue red sky", "fred blue"]
    );
    assert_eq!(titles_matching("noneOf", "red"), ["blue", ""]);
    assert_eq!(
        titles_matching("beginsWith", "red"),
        ["reddish", "red-car", "RED", "blue red sky", "red fox"]
    );
    assert_eq!(
        titles_matching("endsWith", "red"),
        ["Fred", "RED", "blue red sky", "red fox", "fred blue"]
    );
    assert_eq!(titles_matching("words", "red blue"), ["blue red sky"]);
    assert_eq!(titles_matching("empty", ""), [""]);
    assert_eq!(titles_matching("notEmpty", "").len(), 8);
}

#[test]
fn unmeasured_text_cases_follow_the_documented_choices() {
    // Punctuation stays part of a word; accents aren't folded; Unicode
    // letters are lower-cased.
    assert_eq!(titles_matching("words", "red,"), Vec::<&str>::new());
    let entries = [
        Entry {
            title: "Ærøskøbing",
            ..Entry::default()
        },
        Entry {
            title: "Aeroskobing",
            ..Entry::default()
        },
    ];
    assert_eq!(
        matching(&one_rule("title", "any", text("ÆRØ")), &entries),
        [0]
    );
    // Every word of a several-word beginsWith must begin some word.
    assert_eq!(titles_matching("beginsWith", "red fo"), ["red fox"]);
    // An empty value matches everything but empty/notEmpty.
    assert_eq!(titles_matching("any", "").len(), TITLES.len());
}

#[test]
fn keywords_folders_and_collections_match_whole_lists() {
    // The session lists ancestors, synonyms and keywords left out of export
    // as the photo's keyword terms.
    let tagged = Entry {
        keyword_terms: "Parent\nChild\nred-car\nSecretCedar\nAliasJuniper",
        folder: "/Users/someone/issue-998/photos",
        collections: "OliveNeedle\nP3 Static A",
        ..Entry::default()
    };
    let bare = Entry::default();
    let entries = [tagged, bare];
    for value in ["Parent", "Child", "SecretCedar", "AliasJuniper"] {
        assert_eq!(
            matching(&one_rule("keywords", "any", text(value)), &entries),
            [0]
        );
    }
    assert_eq!(
        matching(&one_rule("keywords", "empty", None), &entries),
        [1]
    );
    assert_eq!(
        matching(&one_rule("keywords", "words", text("red")), &entries),
        Vec::<usize>::new()
    );
    assert_eq!(
        matching(&one_rule("folder", "any", text("issue-998")), &entries),
        [0]
    );
    // Folders' names are words too; contains sees the whole path.
    for (operation, value) in [
        ("words", "photos"),
        ("any", "998/photos"),
        ("beginsWith", "iss"),
    ] {
        assert_eq!(
            matching(&one_rule("folder", operation, text(value)), &entries),
            [0],
            "{operation} {value}"
        );
    }
    assert_eq!(
        matching(
            &one_rule("collection", "words", text("OliveNeedle")),
            &entries
        ),
        [0]
    );
    assert_eq!(
        matching(&one_rule("collection", "noneOf", text("Olive")), &entries),
        [1]
    );
}

#[test]
fn rating_zero_is_unrated_and_ranges_take_either_order() {
    let entries: Vec<Entry<'_>> = (0..=5)
        .map(|rating| Entry {
            rating,
            ..Entry::default()
        })
        .collect();
    assert_eq!(matching(&one_rule("rating", "==", num(0.)), &entries), [0]);
    assert_eq!(matching(&one_rule("rating", ">=", num(5.)), &entries), [5]);
    assert_eq!(
        matching(&one_rule("rating", "!=", num(0.)), &entries),
        [1, 2, 3, 4, 5]
    );
    let mut range = Rule::new("rating", "in", num(4.));
    range.value2 = num(2.);
    let group = Group::new(Combine::Intersect, vec![Item::Rule(range)]);
    assert_eq!(matching(&group, &entries), [2, 3, 4]);
    // A stale value2 means nothing outside a range.
    let mut stale = Rule::new("rating", "==", num(3.));
    stale.value2 = num(5.);
    let group = Group::new(Combine::Intersect, vec![Item::Rule(stale)]);
    assert_eq!(matching(&group, &entries), [3]);
}

fn captured(times: &[&'static str]) -> Vec<Entry<'static>> {
    times
        .iter()
        .map(|captured| Entry {
            captured,
            ..Entry::default()
        })
        .collect()
}

#[test]
fn dates_are_calendar_days_as_measured() {
    let entries = captured(&[
        "2026-11-17T09:07:10.430",
        "2026-11-16T01:00:00.000",
        "2026-11-16T23:59:00.000",
        "2026-11-15T12:00:00.000",
        "2026-11-14T12:00:00.000",
    ]);
    let mut in_last = Rule::new("captureTime", "inLast", num(1.));
    in_last.units = Some("days".into());
    let in_last = Group::new(Combine::Intersect, vec![Item::Rule(in_last)]);
    assert_eq!(matching(&in_last, &entries), [0, 1, 2]);
    assert_eq!(
        matching(&one_rule("captureTime", "today", None), &entries),
        [0]
    );
    assert_eq!(
        matching(&one_rule("captureTime", "yesterday", None), &entries),
        [1, 2]
    );
    // Sunday the 15th starts the week; Saturday the 14th is the week before.
    assert_eq!(
        matching(&one_rule("captureTime", "thisWeek", None), &entries),
        [0, 1, 2, 3]
    );
    assert_eq!(
        matching(&one_rule("captureTime", "thisMonth", None), &entries).len(),
        5
    );
    assert_eq!(
        matching(&one_rule("captureTime", "==", text("2026-11-16")), &entries),
        [1, 2]
    );
    assert_eq!(
        matching(&one_rule("captureTime", ">", text("2026-11-15")), &entries),
        [0, 1, 2]
    );
    assert_eq!(
        matching(&one_rule("captureTime", "<", text("2026-11-15")), &entries),
        [4]
    );
    // A Monday week start (another locale) leaves Sunday out.
    let monday = Clock {
        week_start: 1,
        ..clock()
    };
    let matcher = one_rule("captureTime", "thisWeek", None)
        .compile(&monday)
        .unwrap();
    let week: Vec<usize> = (0..entries.len())
        .filter(|i| matcher.matches(&entries[*i]))
        .collect();
    assert_eq!(week, [0, 1, 2]);
}

#[test]
fn months_and_years_back_keep_the_day_or_the_months_last() {
    let today = Date::from_ymd(2026, 3, 31);
    assert_eq!(today.months_back(1), Date::from_ymd(2026, 2, 28));
    assert_eq!(today.months_back(12), Date::from_ymd(2025, 3, 31));
    assert_eq!(
        Date::from_ymd(2024, 2, 29).months_back(12),
        Date::from_ymd(2023, 2, 28)
    );
    assert_eq!(Date::from_ymd(2026, 11, 17).weekday(), 2);
    assert_eq!(Date::parse("2026-02-30"), None);
    assert_eq!(
        Date::parse("2026-11-17T09:07"),
        Some(Date::from_ymd(2026, 11, 17))
    );
}

#[test]
fn a_photo_without_a_date_matches_no_date_rule() {
    let entries = captured(&["", "garbage", "2026-11-17T09:07:10"]);
    for (op, value) in [("==", text("2026-11-17")), ("!=", text("2026-11-01"))] {
        assert_eq!(
            matching(&one_rule("captureTime", op, value), &entries),
            [2],
            "{op}"
        );
    }
    let mut not_in_last = Rule::new("captureTime", "notInLast", num(30.));
    not_in_last.units = Some("days".into());
    let group = Group::new(Combine::Intersect, vec![Item::Rule(not_in_last)]);
    assert_eq!(matching(&group, &entries), Vec::<usize>::new());
    // The edit date comes from the session.
    let edited = [
        Entry {
            edited: Some(Date::from_ymd(2026, 11, 16)),
            ..Entry::default()
        },
        Entry::default(),
    ];
    let mut recent = Rule::new("touchTime", "inLast", num(2.));
    recent.units = Some("days".into());
    let group = Group::new(Combine::Intersect, vec![Item::Rule(recent)]);
    assert_eq!(matching(&group, &edited), [0]);
    assert!(group.needs_edit_facts());
}

#[test]
fn combines_and_nested_groups_follow_lightroom() {
    // Lightroom saved this with every rule `rating >= 0`; it listed no photos.
    let nested = Group::from_lightroom(
        "s = {\n\t{\n\t\tcriteria = \"rating\",\n\t\toperation = \">=\",\n\t\tvalue = 0,\n\t},\n\t{\n\t\t{\n\t\t\tcriteria = \"rating\",\n\t\t\toperation = \">=\",\n\t\t\tvalue = 0,\n\t\t},\n\t\t{\n\t\t\t{\n\t\t\t\tcriteria = \"rating\",\n\t\t\t\toperation = \">=\",\n\t\t\t\tvalue = 0,\n\t\t\t},\n\t\t\tcombine = \"intersect\",\n\t\t},\n\t\t{\n\t\t\t{\n\t\t\t\tcriteria = \"rating\",\n\t\t\t\toperation = \">=\",\n\t\t\t\tvalue = 0,\n\t\t\t},\n\t\t\tcombine = \"exclude\",\n\t\t},\n\t\tcombine = \"union\",\n\t},\n\tcombine = \"exclude\",\n}\n",
    )
    .unwrap();
    let entries = [
        Entry::default(),
        Entry {
            rating: 3,
            ..Entry::default()
        },
    ];
    assert_eq!(matching(&nested, &entries), Vec::<usize>::new());
    let rule = |n| Item::Rule(Rule::new("rating", "==", num(n)));
    let any = Group::new(Combine::Union, vec![rule(0.), rule(3.)]);
    assert_eq!(matching(&any, &entries), [0, 1]);
    let all = Group::new(Combine::Intersect, vec![rule(0.), rule(3.)]);
    assert_eq!(matching(&all, &entries), Vec::<usize>::new());
    let none = Group::new(Combine::Exclude, vec![rule(3.)]);
    assert_eq!(matching(&none, &entries), [0]);
}

#[test]
fn labels_flags_and_file_types_use_lightroom_values() {
    let entries = [
        Entry {
            label: "Red",
            flag: 1,
            filename: "a.CR3",
            ..Entry::default()
        },
        Entry {
            label: "Purple",
            flag: -1,
            filename: "b.dng",
            ..Entry::default()
        },
        Entry {
            label: "Approved",
            filename: "c.jpeg",
            ..Entry::default()
        },
        Entry {
            filename: "d.HEIF",
            ..Entry::default()
        },
    ];
    assert_eq!(
        matching(&one_rule("labelColor", "==", num(1.)), &entries),
        [0]
    );
    assert_eq!(
        matching(&one_rule("labelColor", "==", num(5.)), &entries),
        [1]
    );
    assert_eq!(
        matching(&one_rule("labelColor", "==", text("custom")), &entries),
        [2]
    );
    assert_eq!(
        matching(&one_rule("labelColor", "==", text("none")), &entries),
        [3]
    );
    assert_eq!(
        matching(&one_rule("labelColor", "!=", text("none")), &entries),
        [0, 1, 2]
    );
    assert_eq!(matching(&one_rule("pick", "==", num(-1.)), &entries), [1]);
    assert_eq!(matching(&one_rule("pick", "==", num(0.)), &entries), [2, 3]);
    assert_eq!(
        matching(&one_rule("labelText", "==", text("approved")), &entries),
        [2]
    );
    assert_eq!(
        matching(&one_rule("labelText", "empty", None), &entries),
        [3]
    );
    for (format, expected) in [("RAW", 0), ("DNG", 1), ("JPG", 2), ("HEIC", 3)] {
        assert_eq!(
            matching(&one_rule("fileFormat", "==", text(format)), &entries),
            [expected]
        );
    }
    assert_eq!(
        matching(&one_rule("fileFormat", "==", text("VIDEO")), &entries),
        Vec::<usize>::new()
    );
}

#[test]
fn shutter_and_aperture_rules_hold_apex_values() {
    let info = |exposure, aperture| PhotoInfo {
        exposure: Some(exposure),
        aperture: Some(aperture),
        focal: Some(50.),
        iso: Some(400.),
        dimensions: Some((4000, 6000)),
        ..PhotoInfo::default()
    };
    let infos = [info(0.01, 0.1), info(0.004, 2.8)];
    let entries: Vec<Entry<'_>> = infos
        .iter()
        .map(|info| Entry {
            info: Some(info),
            ..Entry::default()
        })
        .collect();
    // As Lightroom saved 1/100 sec and f/0.10.
    assert_eq!(
        matching(
            &one_rule("shutterSpeed", "==", num(6.6438561897747)),
            &entries
        ),
        [0]
    );
    assert_eq!(
        matching(
            &one_rule("shutterSpeed", ">", num(6.6438561897747)),
            &entries
        ),
        [1]
    );
    assert_eq!(
        matching(&one_rule("aperture", "==", num(-6.6438561897747)), &entries),
        [0]
    );
    assert_eq!(
        matching(&one_rule("focalLength", "==", num(50.)), &entries),
        [0, 1]
    );
    assert_eq!(
        matching(&one_rule("megapixels", ">=", num(24.)), &entries),
        [0, 1]
    );
    assert_eq!(
        matching(&one_rule("longEdge", "==", num(6000.)), &entries),
        [0, 1]
    );
    assert_eq!(
        matching(&one_rule("aspectRatio", "==", text("portrait")), &entries),
        [0, 1]
    );
    // No photo info: no numeric rule matches, `!=` included.
    let bare = [Entry::default()];
    assert_eq!(
        matching(&one_rule("isoSpeedRating", "!=", num(100.)), &bare),
        Vec::<usize>::new()
    );
}

#[test]
fn edit_facts_answer_flag_and_treatment_rules() {
    let entries = [
        Entry {
            edit: EditFacts {
                adjusted: true,
                cropped: true,
                grayscale: true,
            },
            ..Entry::default()
        },
        Entry::default(),
    ];
    assert_eq!(
        matching(
            &one_rule("hasAdjustments", "isTrue", Some(Value::Bool(true))),
            &entries
        ),
        [0]
    );
    assert_eq!(
        matching(
            &one_rule("cropped", "isFalse", Some(Value::Bool(true))),
            &entries
        ),
        [1]
    );
    assert_eq!(
        matching(&one_rule("treatment", "==", text("grayscale")), &entries),
        [0]
    );
    assert!(one_rule("cropped", "isTrue", None).needs_edit_facts());
    assert!(!one_rule("rating", "==", num(1.)).needs_edit_facts());
}

#[test]
fn searchable_text_covers_the_fields_rawmakase_keeps() {
    let entry = Entry {
        filename: "IMG_1.CR3",
        folder: "/trips/Kyoto",
        title: "Temple",
        keyword_terms: "Japan",
        ..Entry::default()
    };
    let entries = [entry, Entry::default()];
    for value in ["img_1", "kyoto", "temple", "japan"] {
        assert_eq!(
            matching(&one_rule("all", "any", text(value)), &entries),
            [0],
            "{value}"
        );
    }
    assert_eq!(
        matching(&one_rule("metadata", "any", text("temple")), &entries),
        [0]
    );
    assert_eq!(
        matching(&one_rule("metadata", "any", text("kyoto")), &entries),
        Vec::<usize>::new()
    );
}

#[test]
fn rules_rawmakase_cannot_answer_are_unsupported() {
    for (criteria, operation, value) in [
        ("likes", "isTrue", Some(Value::Bool(true))),
        ("widthCropped", "==", num(1000.)),
        ("fileFormat", "==", text("DNG-LOSSY")),
        ("labelColor", "==", num(7.)),
        ("title", "==", text("x")),
        ("filename", "words", text("x")),
        ("captureTime", "inLast", num(1.)),
        ("captureTime", "==", text("yesterday")),
        ("rating", ">=", text("5")),
        ("someFutureField", "==", num(1.)),
    ] {
        let group = one_rule(criteria, operation, value);
        let why = group.unsupported().expect(criteria);
        assert_eq!(why.criteria, criteria);
    }
    let likes = one_rule("likes", "isTrue", Some(Value::Bool(true)))
        .unsupported()
        .unwrap();
    assert_eq!(
        likes.to_string(),
        "Has Likes: RAWmakase doesn't keep what this rule asks about"
    );
    // One unsupported rule anywhere makes the whole collection unsupported.
    let nested = Group::new(
        Combine::Union,
        vec![
            Item::Rule(Rule::new("rating", "==", num(5.))),
            Item::Group(one_rule("hasAI", "isTrue", Some(Value::Bool(true)))),
        ],
    );
    assert_eq!(nested.unsupported().unwrap().criteria, "hasAI");
}

#[test]
fn lightroom_default_smart_collections_are_supported() {
    for text in [
        "s = {\n\t{\n\t\tcriteria = \"labelColor\",\n\t\toperation = \"==\",\n\t\tvalue = 1,\n\t},\n\tcombine = \"intersect\",\n}\n",
        "s = {\n\t{\n\t\tcriteria = \"rating\",\n\t\toperation = \">=\",\n\t\tvalue = 5,\n\t},\n\tcombine = \"intersect\",\n}\n",
        "s = {\n\t{\n\t\tcriteria = \"captureTime\",\n\t\toperation = \"inLast\",\n\t\tvalue = 1,\n\t\tvalue_units = \"months\",\n\t},\n\tcombine = \"intersect\",\n}\n",
        "s = {\n\t{\n\t\tcriteria = \"touchTime\",\n\t\toperation = \"inLast\",\n\t\tvalue = 2,\n\t\tvalue_units = \"days\",\n\t},\n\tcombine = \"intersect\",\n}\n",
        "s = {\n\t{\n\t\tcriteria = \"fileFormat\",\n\t\toperation = \"==\",\n\t\tvalue = \"VIDEO\",\n\t},\n\tcombine = \"intersect\",\n}\n",
        "s = {\n\t{\n\t\tcriteria = \"keywords\",\n\t\toperation = \"empty\",\n\t},\n\tcombine = \"intersect\",\n}\n",
    ] {
        let group = Group::from_lightroom(text).unwrap();
        assert_eq!(group.unsupported(), None, "{text}");
        assert_eq!(group.to_lightroom(), text);
    }
}

/// Documented choices the review asked to see tested: a date after today is
/// in the last of any period, several words in endsWith must each end a word,
/// and Copy Name is the copy name as stored, a promoted master's included.
#[test]
fn future_dates_several_word_ends_and_copy_names() {
    let entries = captured(&["2026-11-20T10:00:00", "2026-11-10T10:00:00"]);
    let mut in_last = Rule::new("captureTime", "inLast", num(2.));
    in_last.units = Some("days".into());
    let in_last = Group::new(Combine::Intersect, vec![Item::Rule(in_last)]);
    assert_eq!(matching(&in_last, &entries), [0]);
    assert_eq!(titles_matching("endsWith", "red sky"), ["blue red sky"]);
    let copies = [
        Entry {
            copy_name: "Copy 2",
            ..Entry::default()
        },
        Entry::default(),
    ];
    assert_eq!(
        matching(&one_rule("copyname", "notEmpty", None), &copies),
        [0]
    );
    assert_eq!(
        matching(&one_rule("copyname", "any", text("copy")), &copies),
        [0]
    );
    assert_eq!(matching(&one_rule("copyname", "empty", None), &copies), [1]);
}

/// GPS Data: a location the catalog keeps or cleared answers it; one it
/// doesn't know matches neither is nor is not.
#[test]
fn gps_data_rules_need_a_known_location() {
    let entries = [
        Entry {
            gps: Some(true),
            ..Entry::default()
        },
        Entry {
            gps: Some(false),
            ..Entry::default()
        },
        Entry::default(),
    ];
    let yes = Some(Value::Bool(true));
    assert_eq!(
        matching(&one_rule("hasGPSData", "==", yes.clone()), &entries),
        [0]
    );
    assert_eq!(matching(&one_rule("hasGPSData", "!=", yes), &entries), [1]);
    assert_eq!(
        matching(
            &one_rule("hasGPSData", "==", Some(Value::Bool(false))),
            &entries
        ),
        [1]
    );
    assert!(
        one_rule("hasGPSData", "==", num(1.))
            .unsupported()
            .is_some()
    );
}
