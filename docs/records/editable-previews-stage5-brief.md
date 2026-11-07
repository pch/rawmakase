# Editable previews, Stage 5 brief

Status: Plan (2026-11-07), not built. The brief for Stage 5 of the
[editable previews plan](../editable-previews.md): editing an offline photo from its
editable preview. Code anchors are as of `main` at a84cb7f and will move.

Start after Stage 4 (Build and Discard Editable Previews) is merged into
`GrogHills/rawmakase-fork` `main`. Base the work on that `main`. Anchors below are
from `main` at a84cb7f (2026-11-07); re-check them, Stage 4 will have moved some.

## Read first
- AGENTS.md, all of it (lifecycle, save-before-navigation, tests, review, commits).
- `docs/editable-previews.md` on main: Stage 5, and decisions 2 (identity read when
  building), 4 (AI masks and AI Denoise disabled offline), 5 (no export from
  editable previews), 8 (Camera Look: default fit kept, manual fit disabled).
- docs/architecture.md, docs/code-map.md, docs/catalogs.md (editable previews
  section, added by Stage 4), docs/lightroom-profiles.md (Camera Look).
- Stage 4's PR, for the API it added (build/discard, the stored Camera Look section
  reader) and anything it deferred to Stage 5.

## Goal
A catalog photo whose original is offline but which has a usable editable preview
opens in Develop (and the Loupe) from the preview. Its edit is the photo's edit:
saved in the catalog against the ORIGINAL's identity (recorded in the preview),
with History, autosave and save-before-navigation working as for any photo. When
the original is back, the photo opens from the original again, with the same edit.

## What blocks it today (verified on a84cb7f)
1. Refusal: `develop_refusal` (src/app/library/mod.rs:1523) refuses offline photos;
   `develop_source` (mod.rs:592) also reads the file (`Stamp::read`). Callers:
   src/app/catalog/mod.rs:649 (`develop_catalog_photo`), src/app/develop/workflow.rs:629
   (`develop_step`, arrow-key stepping), src/app/shell/commands.rs:1149 (automation
   `open`), src/app/library/cell.rs:557 (menu), src/app/library/info.rs:29/51 (Quick
   Develop and its "Offline: relink…" note), src/app/develop/reference.rs:117/156.
2. Identity: the loader reads `Identity::read(&path).ok()` (src/app/worker/loader.rs:255);
   offline it is `None`, so `flush`/autosave fail with `UNIDENTIFIED`
   (workflow.rs:25/203/276). `Start::capture` → `EditRecord::saved(path)`
   (crates/rawmakase-interop/src/edits.rs:82) reads the missing file, so the edit
   opens protected. `Catalog::check_edit` (crates/rawmakase-catalog/src/catalog/edit_rows.rs:65)
   reads the file at :75 and again through `load_edit` at :85.
3. "fast means draft": a reduced image is treated as a draft to replace —
   src/app/file_ops/mod.rs:1601 reopens the photo when the full image is `fast`;
   the loader sends a Half then submits a `FullJob` (loader.rs:385); prefetch of a
   neighbour skips offline ones (workflow.rs:138); AI Denoise silently skips `fast`
   (src/app/develop/ai_denoise.rs:119); export re-decodes `fast` images
   (crates/rawmakase-export/src/export/job.rs:281). `CameraImage::photo_scale`
   (camera_data.rs:486) relies on `fast` and must keep working for editable previews.
4. Camera Look default fit: `camera_look::with_default` keys its cache with
   `FitKey::of(path)` (crates/rawmakase-decode/src/camera_look/mod.rs:241/257), so it
   finds nothing offline. Callers: loader.rs:271, src/app/library/preview_build.rs:206,
   src/app/library/previews/source.rs:28, src/app/library/sync.rs:498/651,
   crates/rawmakase-export/src/export/batch.rs:508, src/main.rs:648.
5. Reference View (src/app/worker/reference.rs:110/122) and the Loupe
   (src/app/library/loupe.rs:336/353 `loupe_develops` requires `is_available`;
   src/app/develop/stand_in.rs:224) assume an online original.

## Design (decide the details; keep owners as the plan's Ownership table says)
- `DevelopSource` names its source: the original, or the editable preview (its file,
  the recorded original `Identity`, sizes). `develop_refusal` returns `Offline` only
  when there is no usable (present, readable, not stale) editable preview.
- The loader opens the preview with `editable_preview::read`; the document's
  identity for saving is the recorded original identity, never the preview file's.
  No `FullJob` and no neighbour prefetch for it.
- Saving offline goes through the catalog's own checks (one owner, `edit_rows`):
  `check_edit` and `load_edit` take the recorded identity instead of reading the
  missing file; `EditRecord::saved` gets the matching variant. The stored identity
  stays an ordinary `Identity`, so older releases read it as they do now.
  Refuse an offline save when the catalog's stored edit identity is not
  `same_photo` as the preview's recorded one (the preview is of another version).
- Mark "this is an editable preview, final, don't replace it" explicitly, so the
  draft-replacement paths (file_ops reopen, loader FullJob, export re-decode) don't
  fire, while `photo_scale` keeps working. Choose where that lives (a field on the
  loaded document/source is likely better than a second meaning of `fast`) and say
  why.
- Camera Look: when the cache can't be used (offline), open with the fit stored
  in the preview (Stage 4 writes it for `Absent` photos). Disable Profile › Fit
  Camera Look on an editable preview: src/app/develop/camera_look.rs:116
  `can_fit_camera_look`, button at
  src/app/develop/panels/inspector/panels/basic.rs:193 (add disabled hover text
  "Connect the original to fit a Camera Look").
- AI masks disabled: src/app/develop/tools/subject_mask/mod.rs:350
  `selection_unavailable()` gains "Connect the original to use AI masks" (uses at
  :371/413/454, ui.rs:27/552/563/586/592). AI Denoise disabled with a visible reason
  (checkbox at src/app/develop/panels/inspector/ai_denoise.rs:33; request gate
  ai_denoise.rs:118–119). Masks and AI results already saved render as usual.
- HDR stays available (the preview keeps headroom; `check_recipe` path, hdr.rs:169).
- Export stays refused offline (src/app/export/mod.rs:252); keep it.
- Display: the status line/readout says "Editable Preview" instead of "Original"
  (loader.rs:292 sets "Original"; src/app/shell/workspace.rs:1774), Develop and the
  Loupe show it; the Library gets a "Has Editable Preview" filter
  (src/app/library/filter.rs:73; `photos_with_editable_previews`).
- Automation `open` (commands.rs:1127/1149) opens from the preview too, and its
  state reports which source is open.
- When the original comes back, the next open uses it. If it returns while the
  photo is open from its preview, keep editing the preview until reopened; saves
  must still check identity correctly.

## Tests (deterministic, synthetic files; test at the editor/catalog boundary)
- Open offline from a preview; save; reconnect the same file → the edit loads.
- Reconnect a replaced file → the edit is protected.
- Preview deleted while open; preview stale (original changed) → refused with reason.
- Failed save offline stays pending and blocks navigation (save-before-navigation).
- Original returns while open from the preview.
- No FullJob / no file_ops reopen / no export re-decode for a preview.
- AI masks, AI Denoise and Fit Camera Look disabled with their reasons; saved
  masks and fits still render.
- Camera Look default from the stored fit when offline; same look as online.
- Arrow-key stepping, Quick Develop, menu and automation `open` all accept a photo
  with a preview and refuse one without.
- Rendering: an editable preview renders within the existing Fit bound of the
  original resized (sharpening, grain and noise reduction follow `photo_scale`,
  stages 1a and 1b).

## Process
`CARGO_INCREMENTAL=0 make check` (target/ passes 17 GB; delete
target/*/incremental if the disk fills). Review rounds per AGENTS.md (no Codex
here: fresh Claude reviewers, labelled a weaker substitute), split by risk area:
save/identity consistency, lifecycle (load, cancel, stale results, worker loss),
save-before-navigation, UI/automation translation. Open the PR into fork main;
don't merge. Docs: catalogs.md (the user-facing part), code-map.md,
architecture.md if ownership changes, and the plan's status line.
