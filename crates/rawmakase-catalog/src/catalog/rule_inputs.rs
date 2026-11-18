//! What smart collection rules read of every photo, in a few queries rather
//! than one per photo: the default language of each title and caption, the
//! creators, the keyword tree with each photo's keywords, and what each
//! saved edit does. The session assembles them into
//! `collection_rules::Entry` values.
use super::db::{Reads, sql};
use super::value::row;
use super::{Catalog, PhotoId};
use crate::metadata::DEFAULT_LANG;
use anyhow::Result;
use rawmakase_model::collection_rules::EditFacts;
use std::collections::HashMap;

/// A photo's searchable descriptive text, as rules compare it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuleTexts {
    /// The default language's title, else the first language's.
    pub title: String,
    pub caption: String,
    pub copyright: String,
    /// In order.
    pub creators: Vec<String>,
    /// Whether it has a GPS location: `Some(false)` when cleared, `None`
    /// when the catalog keeps none, its file's own being unknown here.
    pub gps: Option<bool>,
}

/// A keyword of the catalog's tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeywordNode {
    pub id: i64,
    pub name: String,
    pub parent: Option<i64>,
}

impl Catalog {
    /// Every photo's title, caption, copyright, creators and GPS location
    /// that are set; a photo with
    /// none is left out.
    pub fn rule_texts(&self) -> Result<HashMap<PhotoId, RuleTexts>> {
        row! {
            struct Text {
                photo: PhotoId,
                field: String,
                lang: String,
                value: String,
            }
        }
        row! {
            struct Creator {
                photo: PhotoId,
                name: String,
            }
        }
        let texts: Vec<Text> = self.db.read(
            sql!(
                "SELECT photo, field, lang, value FROM photo_text
                 WHERE field IN ('title', 'caption', 'copyright')
                 ORDER BY photo, field, position"
            ),
            &[],
        )?;
        let mut out: HashMap<PhotoId, RuleTexts> = HashMap::new();
        // The first language seen, unless `x-default` comes later.
        let mut chosen_default: HashMap<(PhotoId, String), bool> = HashMap::new();
        for t in texts {
            let is_default = t.lang == DEFAULT_LANG;
            let seen = chosen_default.get(&(t.photo, t.field.clone())).copied();
            if seen == Some(true) || (seen == Some(false) && !is_default) {
                continue;
            }
            chosen_default.insert((t.photo, t.field.clone()), is_default);
            let entry = out.entry(t.photo).or_default();
            match t.field.as_str() {
                "title" => entry.title = t.value,
                "caption" => entry.caption = t.value,
                _ => entry.copyright = t.value,
            }
        }
        let creators: Vec<Creator> = self.db.read(
            sql!("SELECT photo, name FROM photo_creators ORDER BY photo, position"),
            &[],
        )?;
        for c in creators {
            out.entry(c.photo).or_default().creators.push(c.name);
        }
        row! {
            struct Located {
                photo: PhotoId,
                at: bool,
            }
        }
        let located: Vec<Located> = self.db.read(
            sql!(
                "SELECT photo, (cleared = 0 AND lat IS NOT NULL AND lon IS NOT NULL)
                 FROM photo_location"
            ),
            &[],
        )?;
        for l in located {
            out.entry(l.photo).or_default().gps = Some(l.at);
        }
        Ok(out)
    }
    /// Every keyword, with its parent.
    pub fn keyword_tree(&self) -> Result<Vec<KeywordNode>> {
        row! {
            struct Node {
                id: i64,
                name: String,
                parent: Option<i64>,
            }
        }
        let nodes: Vec<Node> = self.db.read(
            sql!("SELECT id, name, parent FROM keywords ORDER BY id"),
            &[],
        )?;
        Ok(nodes
            .into_iter()
            .map(|n| KeywordNode {
                id: n.id,
                name: n.name,
                parent: n.parent,
            })
            .collect())
    }
    /// Every photo's keywords, as (photo, keyword id).
    pub fn photo_keyword_ids(&self) -> Result<Vec<(PhotoId, i64)>> {
        row! {
            struct Tag {
                photo: PhotoId,
                keyword: i64,
            }
        }
        let tags: Vec<Tag> = self.db.read(
            sql!("SELECT photo, keyword FROM photo_keywords ORDER BY photo, keyword"),
            &[],
        )?;
        Ok(tags.into_iter().map(|t| (t.photo, t.keyword)).collect())
    }
}

impl Catalog {
    /// What each photo with a saved edit (RAWmakase's, else Lightroom's) does,
    /// for Has Adjustments, Cropped and Treatment rules: a saved edit counts
    /// as adjusted; the crop and Black & White come from RAWmakase's recipe,
    /// else from Lightroom's `HasCrop` and `ConvertToGrayscale`. An edit that
    /// can't be read counts as adjusted, neither cropped nor Black & White.
    pub fn edit_facts(&self) -> Result<HashMap<PhotoId, EditFacts>> {
        row! {
            struct Edit {
                id: PhotoId,
                recipe: Option<String>,
                lightroom: Option<String>,
            }
        }
        let edits: Vec<Edit> = self.db.read(
            sql!(
                "SELECT id, recipe, lightroom_develop FROM photos
                 WHERE recipe IS NOT NULL OR length(COALESCE(lightroom_develop, '')) > 0"
            ),
            &[],
        )?;
        Ok(edits
            .into_iter()
            .map(|e| {
                let mut facts = EditFacts {
                    adjusted: true,
                    ..EditFacts::default()
                };
                if let Some(recipe) = &e.recipe {
                    // The stored form names its profile and calibration, which
                    // only a read through the catalog's assets resolves.
                    let source = super::recipe_assets::ReadsSource(&self.db);
                    if let Ok(recipe) = crate::model::stored_recipe::from_text(&source, recipe) {
                        facts.cropped = recipe.crop != [0., 0., 1., 1.];
                        facts.grayscale =
                            recipe.treatment() == crate::model::recipe::Treatment::BlackWhite;
                    }
                } else if let Some(fields) = e
                    .lightroom
                    .as_deref()
                    .and_then(|t| rawmakase_interop::lr_develop::develop_fields(t).ok())
                {
                    let yes = |key: &str| fields.get(key).is_some_and(|v| v.trim() == "true");
                    facts.cropped = yes("HasCrop");
                    facts.grayscale = yes("ConvertToGrayscale");
                }
                (e.id, facts)
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::TextField;

    #[test]
    fn rule_inputs_are_read_for_every_photo_at_once() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let folder = directory.path().join("photos");
        std::fs::create_dir(&folder)?;
        for name in ["a.jpg", "b.jpg"] {
            image::RgbImage::new(4, 4).save(folder.join(name))?;
        }
        let mut catalog = Catalog::create(&directory.path().join("c.rawmakase"))?;
        catalog.add_folder(&folder)?;
        let photos = catalog.photos()?;
        let (a, b) = (photos[0].id, photos[1].id);
        catalog.set_text(&[a], TextField::Title, "Temple")?;
        catalog.set_text(&[a, b], TextField::Caption, "Kyoto")?;
        catalog.set_creators(&[b], &["Ann".into(), "Bo".into()])?;
        catalog.add_keywords(&[a], &[vec!["Places".into(), "Japan".into()]])?;
        let texts = catalog.rule_texts()?;
        assert_eq!(texts[&a].title, "Temple");
        assert_eq!(texts[&b].caption, "Kyoto");
        assert_eq!(texts[&b].creators, ["Ann", "Bo"]);
        assert_eq!(texts[&b].gps, None);
        let tree = catalog.keyword_tree()?;
        let japan = tree.iter().find(|k| k.name == "Japan").unwrap();
        let places = tree.iter().find(|k| k.name == "Places").unwrap();
        assert_eq!(japan.parent, Some(places.id));
        assert_eq!(catalog.photo_keyword_ids()?, [(a, japan.id)]);
        Ok(())
    }
}
