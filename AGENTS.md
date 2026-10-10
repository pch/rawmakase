# Working on RAWmakase

## Architecture and implementation

- Before changing a module boundary, read `docs/architecture.md`,
  `docs/code-map.md` and the relevant module documentation. Update them when
  ownership or dependency direction changes.
- Keep domain rules independent of UI and transport adapters. Parameter facts,
  editing side effects, decode policies and protocol types each have one
  authoritative owner.
- Enforce invariants through APIs. Session operations should keep edits, history
  and dirty state consistent; callers should not coordinate these through
  unrelated mutable fields.
- Preserve dependency direction and use narrow visibility. Do not weaken
  architecture checks to make a change pass. Prefer concrete types and simple
  boundaries over speculative abstractions.
- Capture job inputs once; use the same inputs for execution and cache identity.
  Reject stale results after document changes, deletion or replacement.
- Keep mechanical refactors separate from behavior changes. Preserve XMP and
  settings compatibility (Lightroom settings mean what they mean in Lightroom;
  saved slider values survive), migrations and unknown fields; old edits render
  with the current engine, not legacy operators. Never re-bless rendering
  references merely to pass a refactor.
- Use the existing task infrastructure. Failure, panic, cancellation and
  disconnection must clear busy state. Document worker shutdown behavior; avoid
  unbounded waits and blanket joins in `Drop`.
- Handle save failures before navigation or termination becomes irreversible.
  A pending save is not a successful save.

## Tests

- Reproduce bugs with a failing regression test before fixing them. Test
  observable behavior at the relevant boundary, including UI or protocol input
  when translation is the risk.
- For lifecycle changes, cover relevant failure and transition cases: stale
  completion, deletion/recreation, cancellation, worker loss and save failure.
  Prefer deterministic tests.
- Run relevant dependency, workspace and rendering checks alongside the required
  CI checks. Report skipped coverage; do not infer correctness or architectural
  isolation from passing tests alone.

## Code review

- Take an adversarial, evidence-driven approach to review: challenge correctness
  claims and try to falsify important assumptions with concrete scenarios and
  targeted checks. Do not manufacture findings to appear thorough.
- Establish the review scope, base revision and intended behavior first. Read
  `docs/architecture.md`, `docs/code-map.md` and relevant module documentation.
  For review-only requests, report findings without modifying code.
- Review affected callers and error paths, not only the diff. Prioritize data
  integrity, compatibility and user-visible behavior over structural preferences.
- Verify the architecture and lifecycle invariants above across affected paths,
  especially XMP and settings compatibility, session consistency, job inputs and
  cache identity, stale results, worker failure and shutdown, and
  save-before-navigation.
- Support each actionable finding with severity, an exact file and line,
  triggering scenario, consequence and evidence or a minimal reproduction.
  Suggest the smallest coherent correction when supported by the evidence.
  Distinguish confirmed bugs, potential risks and pre-existing issues; keep
  design preferences out of the defect list.
- Judge refactors against their intended outcomes. File moves, new structs and
  new crates do not by themselves establish ownership or encapsulation.
- Lead review reports with findings ordered by severity, followed by unresolved
  questions, verification gaps and a brief assessment against the intended
  outcome. If no actionable defects are found, say so and state coverage limits.

## Addressing code review comments

- Evaluate each comment against the code, intended behavior and evidence. Do not
  implement suggestions automatically.
- Fix real defects and make improvements with a clear benefit. Choose the
  smallest coherent solution, even when it differs from the suggested
  implementation.
- Decline suggestions that are incorrect, redundant, purely speculative or add
  complexity without meaningful benefit. Explain the reasoning concretely and
  respectfully.
- When a concern is valid but belongs outside the change, identify a focused
  follow-up rather than silently expanding scope.
- Close the loop: state what changed and how it was verified, or why no change
  is warranted.

## Before every push

Run the checks CI runs (`.github/workflows/ci.yml`) and push only when they
pass. Use the current stable Rust, as CI does; newer clippy releases add lints.

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

A `v*` tag builds and publishes a release, so the same applies before tagging.

## Releases

Follow [the release guide](packaging/RELEASING.md#writing-release-notes).
Write reviewed notes in `packaging/release-notes/vX.Y.Z.md` before tagging.
Stable releases require this file; never leave generated or placeholder notes.

Notes explain what changed since the previous stable release and why it matters
for someone using RAWmakase. Inspect the actual tagged commit range and relevant
code, not just commit titles. Start with a short plain-language summary, then use
only the sections that apply: **New**, **Improved**, **Fixed**, **Changed**, or
**Removed**. Lead each bullet with a bold user-visible result. Do not list internal
refactors, CI repairs or implementation details unless they affect users. Never
include changes still on main or in the working tree but absent from the tag.

Include direct download links, relevant compatibility or upgrade notes, known
limitations, and a full-changelog link. Credit contributors and reporters when
supported by the history; omit empty sections and boilerplate thanks. Include
screenshots for substantial visual changes when they help explain the change,
using only synthetic or explicitly approved content. Upload media as release
assets, never commit private photos or screenshots. Verify every linked asset.
Do not invent benchmarks, compatibility claims, contributor credits or fixes.
No reference project or previous release style is required.

1. Bump the version in `Cargo.toml`, `Cargo.lock`, `packaging/macos/Info.plist` and
   both `packaging/arch/*/PKGBUILD` files (see the previous `Release x.y.z` commit).
2. Write and review the notes, run the required checks, and commit as
   `Release x.y.z`, with a short summary of what changed.
3. Push `main`, verify the release commit is on `origin/main`, then create
   and push its annotated tag `vx.y.z`.
4. Wait for every build, signing/notarization, installation check and publication.
   Upload any linked media and verify the published text, images and downloads.
   A pushed tag alone does not complete a release. GitHub publication is enabled;
   AUR publication remains disabled.

## Commits

- Commit only the files you changed, by explicit path.
- Never commit Adobe profiles, RAW files or paths from your own machine.
