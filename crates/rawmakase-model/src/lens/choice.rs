//! Which lens profile a photo uses: Lightroom's Lens Corrections › Profile Setup, and
//! the profile an edit names, an Adobe one (`crs:LensProfileSetup`,
//! `LensProfileName`, `LensProfileFilename`, `LensProfileDigest`) or a Lensfun lens
//! the user chose ([`LensfunChoice`]), never both.
use crate::camera_data::Metadata;
use crate::lens::lensfun::LensfunChoice;
use crate::optics::lcp::{Candidate, ImportedProfile, PhotoProfiles};
use serde::{Deserialize, Serialize};

/// Lightroom's Setup menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LensProfileSetup {
    /// The profile saved as the lens's default; RAWmakase keeps no saved lens
    /// defaults, so this matches automatically, as Lightroom does without one.
    #[default]
    Default,
    /// The imported profile that fits the photo's lens best.
    Auto,
    /// The profile the user picked, kept even for another lens.
    Custom,
}
impl LensProfileSetup {
    pub const ALL: [Self; 3] = [Self::Default, Self::Auto, Self::Custom];
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Auto => "Auto",
            Self::Custom => "Custom",
        }
    }
    /// The `crs:LensProfileSetup` value.
    pub fn xmp(self) -> &'static str {
        match self {
            Self::Default => "LensDefaults",
            Self::Auto => "Auto",
            Self::Custom => "Custom",
        }
    }
    /// A `crs:LensProfileSetup` value; anything unknown reads as Default.
    pub fn from_xmp(value: &str) -> Self {
        match value {
            "Auto" => Self::Auto,
            "Custom" => Self::Custom,
            _ => Self::Default,
        }
    }
}

/// A profile as an edit records it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LensProfileId {
    /// `crs:LensProfileName`, e.g. "Adobe (Sony FE 55mm F1.8 ZA)".
    pub name: String,
    /// `crs:LensProfileFilename`, the LCP file's name.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub filename: String,
    /// `crs:LensProfileDigest`, Adobe's digest of the profile; kept as read, since
    /// RAWmakase cannot compute it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub digest: String,
    /// `crs:LensProfileIsEmbedded`: the profile is the one the RAW carries (Adobe
    /// names it "Camera Settings"), so the built-in correction renders, not an LCP.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub embedded: bool,
}
impl LensProfileId {
    pub fn of(profile: &ImportedProfile) -> Self {
        Self {
            name: profile.name.clone(),
            filename: profile.filename.clone(),
            digest: String::new(),
            embedded: false,
        }
    }
    /// The name to show: the profile name, else the file name.
    pub fn label(&self) -> &str {
        if self.name.is_empty() {
            &self.filename
        } else {
            &self.name
        }
    }
}

/// The profile an edit names: an Adobe one, as Lightroom records it, or a Lensfun
/// lens. Saved as the Adobe identity's own object, or as `{"lensfun": …}`; reading
/// goes by that key, so a damaged Lensfun choice is refused rather than read as an
/// Adobe profile without a name. Releases before Lensfun never read one: recipes with
/// it are saved in a format they refuse (`saved_format`, the catalog's version 4).
#[derive(Clone, Debug, PartialEq)]
pub enum LensProfileRef {
    Adobe(LensProfileId),
    Lensfun(Box<LensfunChoice>),
}
/// The key a saved Lensfun choice is under.
const LENSFUN_KEY: &str = "lensfun";
impl Serialize for LensProfileRef {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Adobe(id) => id.serialize(s),
            Self::Lensfun(choice) => {
                use serde::ser::SerializeMap;
                let mut map = s.serialize_map(Some(1))?;
                map.serialize_entry(LENSFUN_KEY, choice)?;
                map.end()
            }
        }
    }
}
impl<'de> Deserialize<'de> for LensProfileRef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let mut value = serde_json::Value::deserialize(d)?;
        match value.as_object_mut().and_then(|o| o.remove(LENSFUN_KEY)) {
            Some(lensfun) => {
                let extra = value.as_object().map_or(0, serde_json::Map::len);
                if extra > 0 {
                    return Err(D::Error::custom(
                        "a Lensfun lens profile with Adobe profile fields",
                    ));
                }
                serde_json::from_value(lensfun)
                    .map(|c| Self::Lensfun(Box::new(c)))
                    .map_err(|e| D::Error::custom(format!("Lensfun lens profile: {e}")))
            }
            None => serde_json::from_value(value)
                .map(Self::Adobe)
                .map_err(D::Error::custom),
        }
    }
}
impl LensProfileRef {
    pub fn adobe(&self) -> Option<&LensProfileId> {
        match self {
            Self::Adobe(id) => Some(id),
            Self::Lensfun(_) => None,
        }
    }
    pub fn lensfun(&self) -> Option<&LensfunChoice> {
        match self {
            Self::Lensfun(c) => Some(c),
            Self::Adobe(_) => None,
        }
    }
}

/// The recipe's lens profile Setup and the profile it names.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LensProfileChoice {
    pub setup: LensProfileSetup,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<LensProfileRef>,
}

/// What a choice renders with on one photo.
#[derive(Debug, Default)]
pub struct Resolved<'c, 'p> {
    /// The imported profile used, if any.
    pub used: Option<&'p Candidate>,
    /// The profile the edit names when it isn't imported.
    pub missing: Option<&'c LensProfileId>,
}

impl LensProfileChoice {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    /// The Adobe profile the edit names.
    pub fn adobe(&self) -> Option<&LensProfileId> {
        self.id.as_ref().and_then(LensProfileRef::adobe)
    }
    pub fn adobe_mut(&mut self) -> Option<&mut LensProfileId> {
        match &mut self.id {
            Some(LensProfileRef::Adobe(id)) => Some(id),
            _ => None,
        }
    }
    /// The Lensfun lens the edit chose.
    pub fn lensfun(&self) -> Option<&LensfunChoice> {
        self.id.as_ref().and_then(LensProfileRef::lensfun)
    }
    /// Picking a profile in the Make, Model or Profile menu, which sets Custom.
    pub fn choose(&mut self, profile: &ImportedProfile) {
        self.setup = LensProfileSetup::Custom;
        self.id = Some(LensProfileRef::Adobe(self.id_for(profile)));
    }
    /// The choice taken to photo `m`: a Lensfun lens gets `m`'s camera
    /// ([`LensfunChoice::for_photo`], against `lensfun`); an Adobe one is the same.
    pub fn for_photo(&mut self, m: &Metadata, lensfun: &crate::lens::lensfun::installed::Handle) {
        if let Some(LensProfileRef::Lensfun(c)) = &mut self.id {
            **c = c.for_photo(m, lensfun);
        }
    }
    /// Picking a Lensfun lens: Custom, naming it.
    pub fn choose_lensfun(&mut self, choice: LensfunChoice) {
        self.setup = LensProfileSetup::Custom;
        self.id = Some(LensProfileRef::Lensfun(Box::new(choice)));
    }
    /// The identity to record for `profile`: the edit's own when it names the same
    /// file, which keeps Adobe's digest.
    fn id_for(&mut self, profile: &ImportedProfile) -> LensProfileId {
        self.id
            .take()
            .and_then(|id| match id {
                LensProfileRef::Adobe(id) => Some(id),
                LensProfileRef::Lensfun(_) => None,
            })
            // The camera's own profile is not an imported file, whatever its name.
            .filter(|id| !id.embedded && profile.is(&id.filename, &id.name))
            // A name alone may fit several files: record the one picked.
            .map(|id| LensProfileId {
                filename: profile.filename.clone(),
                ..id
            })
            .unwrap_or_else(|| LensProfileId::of(profile))
    }
    /// The choice as it renders: Default and Auto match alike, and the digest is left out.
    pub fn rendering(&self) -> Self {
        Self {
            setup: match self.setup {
                LensProfileSetup::Default => LensProfileSetup::Auto,
                setup => setup,
            },
            // Adobe's digest names the same file; it does not change the render.
            id: self.id.clone().map(|id| match id {
                LensProfileRef::Adobe(id) => LensProfileRef::Adobe(LensProfileId {
                    digest: String::new(),
                    ..id
                }),
                lensfun => lensfun,
            }),
        }
    }
    /// Picking a Setup. Custom keeps the profile in use; Default and Auto match again,
    /// forgetting a Lensfun lens, which they never choose.
    pub fn set_setup(&mut self, setup: LensProfileSetup, in_use: Option<&ImportedProfile>) {
        self.setup = setup;
        self.id = match setup {
            LensProfileSetup::Custom => match in_use {
                Some(profile) => Some(LensProfileRef::Adobe(self.id_for(profile))),
                None => self.id.take(),
            },
            LensProfileSetup::Default | LensProfileSetup::Auto => None,
        };
    }
    /// The profile this choice uses on a photo. A named profile is used when it is
    /// imported: under Custom whatever lens it was made for, under Default and Auto
    /// when it profiles the photo's lens (else the best match, as Lightroom's
    /// automatic choice). A named profile that isn't imported is reported; Custom
    /// then uses no Adobe profile.
    pub fn resolve<'c, 'p>(
        &'c self,
        profiles: &'p PhotoProfiles,
        m: &Metadata,
    ) -> Resolved<'c, 'p> {
        // The camera's own profile renders as built in; a Lensfun lens is never
        // stood in for by an Adobe profile.
        let named = match &self.id {
            Some(LensProfileRef::Adobe(id)) if id.embedded => return Resolved::default(),
            Some(LensProfileRef::Lensfun(_)) => return Resolved::default(),
            Some(LensProfileRef::Adobe(id)) => Some((id, profiles.find(&id.filename, &id.name))),
            None => None,
        };
        match (self.setup, named) {
            // Found but unable to correct this photo, it is reported as not imported is.
            (LensProfileSetup::Custom, Some((id, Some(found)))) => match found.correction(m) {
                Some(_) => Resolved {
                    used: Some(found),
                    missing: None,
                },
                None => Resolved {
                    used: None,
                    missing: Some(id),
                },
            },
            (LensProfileSetup::Custom, Some((id, None))) => Resolved {
                used: None,
                missing: Some(id),
            },
            (_, Some((_, Some(found))))
                if found.lens_rank.is_some() && found.correction(m).is_some() =>
            {
                Resolved {
                    used: Some(found),
                    missing: None,
                }
            }
            (_, named) => Resolved {
                used: profiles.auto(m),
                // A named profile that isn't imported, or can't correct this photo,
                // stays named and is reported.
                missing: named.and_then(|(id, found)| {
                    found
                        .is_none_or(|c| c.correction(m).is_none())
                        .then_some(id)
                }),
            },
        }
    }
}

/// Lightroom's Make, Model and Profile menus over a photo's profiles.
/// Profiles whose correction does not validate for the photo are left out of the
/// Profile menu and never chosen; they are checked as the menus need them, since a
/// full Adobe library offers thousands.
pub struct ProfileMenus<'p> {
    profiles: &'p PhotoProfiles,
    photo: &'p Metadata,
}
impl<'p> ProfileMenus<'p> {
    pub fn new(profiles: &'p PhotoProfiles, photo: &'p Metadata) -> Self {
        Self { profiles, photo }
    }
    fn sorted(&self) -> impl Iterator<Item = &'p ImportedProfile> + 'p {
        self.profiles.in_menu_order().map(|c| c.profile.as_ref())
    }
    pub fn makes(&self) -> Vec<&'p str> {
        let mut makes: Vec<&str> = self.sorted().map(|p| p.lens_make.as_str()).collect();
        makes.dedup();
        makes
    }
    /// The make's models with a profile that validates; only the chosen make's
    /// profiles are checked, so opening the menu stays quick.
    pub fn models(&self, make: &str) -> Vec<&'p str> {
        let photo = self.photo;
        let mut models: Vec<&str> = self
            .profiles
            .in_menu_order()
            .filter(|c| c.profile.lens_make == make && c.correction(photo).is_some())
            .map(|c| c.profile.lens_model.as_str())
            .collect();
        models.dedup();
        models
    }
    pub fn profiles(&self, make: &str, model: &str) -> Vec<&'p ImportedProfile> {
        let photo = self.photo;
        self.profiles
            .in_menu_order()
            .filter(|c| c.profile.lens_make == make && c.profile.lens_model == model)
            .filter(|c| c.correction(photo).is_some())
            .map(|c| c.profile.as_ref())
            .collect()
    }
    /// What picking a make chooses: its first model's first profile.
    pub fn first_of_make(&self, make: &str) -> Option<&'p ImportedProfile> {
        let photo = self.photo;
        self.profiles
            .in_menu_order()
            .filter(|c| c.profile.lens_make == make)
            .find(|c| c.correction(photo).is_some())
            .map(|c| c.profile.as_ref())
    }
    /// What picking a model chooses: its first profile.
    pub fn first_of_model(&self, make: &str, model: &str) -> Option<&'p ImportedProfile> {
        self.profiles(make, model).first().copied()
    }
}

#[cfg(any(test, feature = "test-support"))]
#[cfg_attr(not(test), allow(dead_code))]
pub mod tests;
