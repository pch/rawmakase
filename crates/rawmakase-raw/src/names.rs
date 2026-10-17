//! The one policy for camera names: the maker's name as RAWmakase spells it,
//! the model without the maker's prefix, the body a regional name is
//! (`facts::name`), and the canonical name of a body sold under several
//! (`facts::canonical`). Camera tables, profiles and saved edits use these.
use crate::info::RawInfo;

/// Makers as files write them (case-insensitive prefixes) and as RAWmakase
/// names them. The first that matches wins.
const MAKERS: &[(&str, &str)] = &[
    ("NIKON", "Nikon"),
    ("SONY", "Sony"),
    ("FUJIFILM", "Fujifilm"),
    ("OLYMPUS", "Olympus"),
    ("OM DIGITAL", "OM Digital"),
    ("LEICA", "Leica"),
    ("PENTAX", "Pentax"),
    ("SAMSUNG", "Samsung"),
    ("EASTMAN KODAK", "Kodak"),
    ("KODAK", "Kodak"),
    ("KONICA MINOLTA", "Minolta"),
    ("MINOLTA", "Minolta"),
    ("HUAWEI", "Huawei"),
    ("LG MOBILE", "LG"),
    ("LGE", "LG"),
    ("PHASE ONE", "Phase One"),
    ("SEIKO EPSON", "Epson"),
    ("SIGMA", "Sigma"),
    ("MAMIYA", "Mamiya"),
    ("SINAR", "Sinar"),
    ("XIAOYI", "YI"),
    ("YI TECHNOLOGY", "YI"),
    ("MOTOROLA", "Motorola"),
    ("GOOGLE", "Google"),
    ("HMD GLOBAL", "Nokia"),
    ("HASSELBLAD", "Hasselblad"),
    ("PARROT", "Parrot"),
    ("CANON", "Canon"),
    ("PANASONIC", "Panasonic"),
    ("RICOH", "Ricoh"),
];

/// The maker `make` names, as RAWmakase spells it; another maker's name is
/// kept, trimmed.
pub(crate) fn make(make: &str) -> String {
    let make = make.trim();
    let upper = make.to_ascii_uppercase();
    MAKERS
        .iter()
        .find(|(prefix, _)| upper.starts_with(prefix))
        .map_or_else(|| make.to_string(), |(_, name)| (*name).to_string())
}

/// `model` without the maker's name in front (as `make` is spelled, or as
/// the file spells it) or "Digital Camera" behind (Canon's "Digital" too),
/// trimmed.
pub(crate) fn model(make: &str, file_make: &str, model: &str) -> String {
    let mut model = strip_suffix(model.trim(), make == "Canon");
    // Fujifilm's series name: "FinePix S5600", "FinePixS2Pro".
    if make == "Fujifilm"
        && let Some(rest) = model.strip_prefix("FinePix")
        && !rest.trim().is_empty()
    {
        model = rest.trim();
    }
    let first = file_make.split_whitespace().next().unwrap_or("");
    for prefix in [make, first] {
        if prefix.is_empty() {
            continue;
        }
        if let Some(rest) = model
            .get(..prefix.len())
            .filter(|p| p.eq_ignore_ascii_case(prefix))
            && model[rest.len()..].starts_with(' ')
        {
            return model[rest.len()..].trim().to_string();
        }
    }
    model.to_string()
}

fn strip_suffix(model: &str, canon: bool) -> &str {
    let suffixes: &[&str] = if canon {
        &[" DIGITAL CAMERA", " DIGITAL"]
    } else {
        &[" DIGITAL CAMERA"]
    };
    for suffix in suffixes {
        if model.len() > suffix.len()
            && model.is_char_boundary(model.len() - suffix.len())
            && model[model.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
        {
            return model[..model.len() - suffix.len()].trim_end();
        }
    }
    model
}

/// Names the camera in `info` from the make and model the file writes: the
/// maker, the model without its prefix, the body a regional name is, and the
/// canonical name.
pub(crate) fn apply(info: &mut RawInfo) {
    let file_make = std::mem::take(&mut info.make);
    info.make = make(&file_make);
    // Ricoh Imaging's Pentax bodies are Pentax's.
    if info.make == "Ricoh"
        && info
            .model
            .get(..7)
            .is_some_and(|p| p.eq_ignore_ascii_case("PENTAX "))
    {
        info.make = "Pentax".into();
    }
    info.model = model(&info.make, &file_make, &info.model);
    if let Some(body) = crate::facts::name(&info.make, &info.model) {
        info.model = body.to_string();
    }
    let [mut m, n] = crate::facts::canonical(&info.make, &info.model)
        .cloned()
        .or_else(|| info.canonical.clone())
        .unwrap_or_else(|| [info.make.clone(), info.model.clone()]);
    if !KNOWN.contains(&m.as_str()) {
        m.clear();
    }
    info.normalized_make = m;
    info.normalized_model = n;
}

/// Makers the canonical names keep; any other's canonical make is empty, as
/// the earlier decoder (LibRaw) left it for makers it did not know, so names
/// RAWmakase saved stay as they were (compat-observed, 2026-10-17).
const KNOWN: &[&str] = &[
    "Apple",
    "Broadcom",
    "Canon",
    "DJI",
    "Epson",
    "Fujifilm",
    "GITUP",
    "GoPro",
    "Google",
    "HTC",
    "Hasselblad",
    "Huawei",
    "Kodak",
    "LG",
    "Leaf",
    "Leica",
    "Mamiya",
    "Minolta",
    "Motorola",
    "Nikon",
    "Nokia",
    "OM Digital",
    "Olympus",
    "OnePlus",
    "Panasonic",
    "Parrot",
    "Pentax",
    "Phase One",
    "RaspberryPi",
    "Ricoh",
    "Samsung",
    "Sigma",
    "Sinar",
    "Sony",
    "Xiaomi",
    "YI",
    "Yuneec",
];

#[cfg(test)]
mod tests {
    #[test]
    fn makers_and_models_are_named_without_their_prefixes() {
        assert_eq!(super::make("NIKON CORPORATION"), "Nikon");
        assert_eq!(super::make("RICOH      "), "Ricoh");
        assert_eq!(super::make("Leica Camera AG"), "Leica");
        assert_eq!(super::make("DJI"), "DJI");
        assert_eq!(super::model("Canon", "Canon", "Canon EOS 5D"), "EOS 5D");
        assert_eq!(super::model("Pentax", "PENTAX", "PENTAX K-7"), "K-7");
        assert_eq!(
            super::model("Nikon", "NIKON CORPORATION", "NIKON D850"),
            "D850"
        );
        assert_eq!(super::model("Sony", "SONY", "ILCE-7M3"), "ILCE-7M3");
        assert_eq!(
            super::model("Leica", "Leica Camera AG", "M9 Digital Camera"),
            "M9"
        );
        assert_eq!(
            super::model("Canon", "Canon", "Canon EOS 350D DIGITAL"),
            "EOS 350D"
        );
        // Minolta's "Digital" is part of the name.
        assert_eq!(
            super::model("Minolta", "KONICA MINOLTA", "ALPHA SWEET DIGITAL"),
            "ALPHA SWEET DIGITAL"
        );
        assert_eq!(
            super::model("Minolta", "Minolta Co., Ltd.", "ALPHA-7 DIGITAL"),
            "ALPHA-7 DIGITAL"
        );
        assert_eq!(
            super::model("Fujifilm", "FUJIFILM", "FinePix S5600  "),
            "S5600"
        );
        assert_eq!(
            super::model("Fujifilm", "FUJIFILM", "FinePixS2Pro"),
            "S2Pro"
        );
        // Ricoh Imaging's Pentax bodies.
        let mut info = crate::info::base(crate::Variant::PefHuffman);
        info.make = "RICOH IMAGING COMPANY, LTD.  ".into();
        info.model = "PENTAX K-3".into();
        super::apply(&mut info);
        assert_eq!((info.make.as_str(), info.model.as_str()), ("Pentax", "K-3"));
        // Only a whole word: "Canonet" is not "Canon" "et".
        assert_eq!(super::model("Canon", "Canon", "Canonet"), "Canonet");
    }

    fn named(make: &str, model: &str) -> [String; 4] {
        let mut info = crate::info::base(crate::Variant::DngUncompressed);
        info.make = make.into();
        info.model = model.into();
        super::apply(&mut info);
        [
            info.make,
            info.model,
            info.normalized_make,
            info.normalized_model,
        ]
    }

    #[test]
    fn pentax_bodies_of_ricoh_imaging_are_pentax() {
        assert_eq!(
            named("RICOH IMAGING COMPANY, LTD.  ", "PENTAX K-3 Mark III  "),
            ["Pentax", "K-3 Mark III", "Pentax", "K-3 Mark III"]
        );
        assert_eq!(
            named("RICOH IMAGING COMPANY, LTD.", "RICOH GR III"),
            ["Ricoh", "GR III", "Ricoh", "GR III"]
        );
    }

    #[test]
    fn makers_without_canonical_names_have_an_empty_canonical_make() {
        assert_eq!(
            named("asus", "ASUS_I01WD"),
            ["asus", "ASUS_I01WD", "", "ASUS_I01WD"]
        );
        assert_eq!(
            named("Parrot", "Anafi"),
            ["Parrot", "Anafi", "Parrot", "Anafi"]
        );
    }
}
