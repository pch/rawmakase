# Glossary

Status: Current. The words the code, its comments and these pages use, with where
each lives. Where a word means more than one thing, every meaning is listed.

## Edits

**Recipe** (`model::recipe::Recipe`, crates/rawmakase-model). A photo's develop
settings as saved: every slider, curve, profile choice and lens setting, with the
local edits (spots, red eye, masks) saved beside it as `LocalEdits`. Settings from a
newer release that this one doesn't know are kept in `Recipe::unknown` and written
back, so an older build never drops them. The settings earlier releases used to pick
a rendering engine are dropped instead (`saved_format::OBSOLETE_SETTINGS`).

**ValidRecipe** (`model::valid`). A recipe that passed `Recipe::validate` (finite
values, ranges). Render entry points take one, so nothing renders an unchecked recipe.

**Saved format, schema** (`model::saved_format`). The version a saved recipe is
written as, and the migration of older ones. A recipe is written with the oldest
version that can hold it (`schema_for`): a recipe using a newer feature (a Lensfun
lens, AI Denoise) is written with a newer version, so older releases refuse it
rather than render it without that feature. Catalog edits, native presets and legacy
sidecars all go through it.

**Raw defaults** (`raw_defaults`, crates/rawmakase-interop). Lightroom Classic's
Preferences › Presets › Raw Defaults: the settings a photo without an edit starts
from, and what Reset returns to. A master choice (Adobe Default, Camera Settings,
RAWmakase Default or a preset), with per-camera overrides. Only photos without a
RAWmakase or Lightroom edit follow them.

**Starting edit** (`edits::resolve`, crates/rawmakase-interop). The edit a photo
develops with: its saved RAWmakase edit, else its Lightroom edit, else the raw
defaults. Develop, Sync Settings and Export resolve it here.

**Lightroom edit**. Develop settings imported from a Lightroom catalog or XMP. They
are relative to Adobe's defaults, so they always convert from Adobe Default.

**Setting group** (`model::settings_groups::SettingGroup`). The smallest unit Copy
Settings, Sync and presets can choose, as Lightroom's Copy Settings dialog lists them.
Every recipe field is either in a group, derived for the photo it is applied to
(white balance gains, Upright's analysis), or the photo's own and never transferred
(orientation, settings from a newer release).

**Panel switch** (`model::panels`). The switch in a Develop panel's header, Lightroom's
`Enable*` flags: a panel switched off keeps its settings but renders as if they were
at their defaults.

**History step, edit frame** (`edit_session`). Develop's History is a list of steps,
each a recipe and a name. A frame of the editor begins with `EditSession::begin`; a
change made during it becomes one step, and a drag, wheel scroll or dial turn
(`Gesture::Held`) becomes one step when it ends.

**Treatment**. Lightroom's Color or Black & White switch
(`develop::black_white`).

## Rendering

**Engine**. The renderer (`develop`, crates/rawmakase-engine). There is one: every
recipe renders with it, whichever release saved it.

**Scene tone stage** ([scene tone stage](scene-tone-stage.md)). Where Exposure and
the Basic panel's tone controls render: on scene values, before the camera profile's
look table and tone curve, as in Camera Raw.

**Measurement copy** (`develop::pipeline`). The photo reduced to a fixed size and
taken at the scene tone stage's input with Exposure at 0, on which the photo's
measures are taken (its levels, keys, haze and the base levels Shadows and Highlights
read). Because every render measures the same copy, a preview, a 100% region and an
export of the same recipe agree.

**Photo measures**. Two things. In the engine, `scene_tone::PhotoMeasures`: what the
scene tone stage measures on the measurement copy. In the model,
`model::measures::PhotoMeasures` (re-exported as `xmp::PhotoMeasures`): the trait through which applying settings that ask for Auto
(Auto Tone, Auto white balance, Auto black & white mix) measures the photo;
`develop::Measures` implements it, so XMP needs no rendering code.

**Decode policy** (`decode::DecodePolicy`, crates/rawmakase-decode). How a full-size
decode uses the decode cache: `Show` (Develop and Reference View: recover highlights
and store), `Prefetch` (a neighbour ahead of time: nothing if cached) and `Export`
(read the cache, never store).

**Decode cache** (`decode_cache`). Developed camera images on disk, keyed by file
identity, demosaic setting and build.

**Pyramid** (`develop::pyramid`). Reduced copies of the photo that Fit and
zoomed-out views render from; 100% regions and export use the full image.

**AI source** (`develop::ai_denoise::AiSource`). A photo's AI Denoise result, as the
camera image the renderer develops in place of the plain one. Each result the
service publishes, partial (the tiles covering the 100% view) or final, is a new
source equal only to itself, so caches keyed by it see the change.

**AI Denoise key** (`ai_denoise`). A job's inputs captured once: the file's
identity, the model's digest, the adapter version, the provider and the build. It
names the result's cache entry too.

## Profiles and looks

**RAWmakase Standard, RAWmakase Color** ([profiles](lightroom-profiles.md#rawmakase-profiles)).
RAWmakase's own profiles for every camera with a colour matrix, made from the matrix
and the DNG default tone curve, with no Adobe data.

**Camera Look (fitted)** (`camera_look`, crates/rawmakase-decode;
[profiles](lightroom-profiles.md#camera-look-fitted)). A look profile fitted to a
RAW's embedded JPEG, so a photo can start out looking as the camera rendered it.
Photos without colour data get one by default.

## Previews

**Stand-in**. Something shown in place of the real thing until it is ready.
Mostly: the stored Standard preview Develop shows while a photo opens, until its
first live render (`app::develop::stand_in`); nothing that reads the photo (samples, the
readout, white balance picking, the histogram, tools, export) ever reads it. The
Library's 640-pixel previews and the camera's embedded JPEG stand in the same way.
Also: the camera's lens data used in place of a missing Adobe lens profile
(`camera_data::Metadata::lens_stand_in`), the EXIF an export writes for a source
without readable EXIF (`export::job::stand_in`), and fake models in tests.

**Standard preview, 1:1 preview** ([catalogs](catalogs.md)). Rendered previews
stored in the catalog's preview cache: Standard (2048 pixels) for showing a photo at
once, 1:1 (full size) for the offline Loupe.

**Edited preview**. The Library's 640-pixel preview of a photo developed with its
edit (`app::library::previews`), as opposed to the embedded preview the camera wrote.

**Offline photo**. A catalog photo whose file can't be reached (an unmounted
drive). The Library still lists it and shows stored previews.

## Files and the catalog

**Identity** (`storage::identity::Identity`). What identifies a photo's file: its
size, modification time and a hash of its first 64 KiB. Caches use all three to
tell that a file was replaced. A saved edit stays bound to the same photo, its size
and first bytes (`Identity::same_photo`): a new modification time alone, as a cloud
mount or `touch` leaves, keeps the edit and is stored with the next save. **Stamp** is the cheaper check, size and modification
time from one stat, for cached previews.

**Virtual copy** (`catalog::copies`). Lightroom's virtual copies: another photo of
the same file with its own edit, metadata and name. The photo it was made from is
the master.

**Receipt** (`catalog::file_ops`). Where the system Trash put a file. A file
operation removes a trashed photo's catalog rows only with a receipt for each of its
files and proof that the source is gone (`DeletionEvidence`).

**Journal** (`catalog::file_ops::journal`). The catalog's record of a file operation
in progress, so an interrupted move, copy or Trash can be finished or undone.

## Background work

**Generation**. Three meanings, all "which version this belongs to":

- A task's generation (`app::task::Task`): each started job gets the next number,
  and a result counts only if its generation is still wanted, so cancelled,
  superseded and duplicate completions are dropped.
- The catalog session's structural generation (`catalog_session`): changes whenever
  photos may have changed place, name or master, so a background result read before
  is dropped.
- The preview cache's renderer generation (`catalog::preview_cache`): bumped when
  rendering changes, so stored thumbnails made before are made again.

**Latest, Mailbox** (`app::worker::latest`). A single-slot queue: pending work is
replaced by the latest request. A `Mailbox` submits to a `Latest` worker without
owning it.

**Stopping** (`app::task`). A worker asked to stop, to wait for at exit within a
deadline ([shutdown](shutdown.md)).

**Edit session, catalog session** (`edit_session`, `catalog_session`). The open
photo's edit with its History and saved state, and the open catalog with what was
read from it, kept apart from the window that shows them.
