# Working on RAWmakase

## GitHub preference

Prefer the GitHub CLI (`gh`, including `gh api`) for GitHub work such as issues,
pull requests, reviews, comments, and media uploads. Check the installed CLI
capabilities before falling back to GitHub connectors or browser automation.

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

## Comments

- Keep what the code can't say: reasons, contracts, units, colour spaces,
  `SAFETY` notes and measured Camera Raw facts.
- Delete comments that only restate a name or a type.
- Rewrite terse, clause-packed comments as plain, full sentences, but only in
  files you are already changing for other work; don't open a file just to
  reword its comments.
- Prefer intra-doc links (``[`Recipe::validate`]``) to plain backticked names,
  so a rename shows up as a broken link when the docs are built.
- There is no target for how many comments a file has, and no blanket removal:
  judge each comment on what it tells the reader.

## Tests

- Before running dataset rendering or Camera Raw parity checks, read
  `tests/corpus/README.md` and, when available, the local handoff at
  `$RAWMAKASE_DATASET/baselines/STRESS-REFERENCES.md`. It records the stress-only
  run commands, reference status, exclusions and coverage limits. This handoff
  is local and Git-ignored; do not assume it exists on another machine or commit
  its private content or machine-specific paths.
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

## Code review with subagents

- Use subagents when a change spans several risk areas or is too large to
  review carefully in one pass. Review small, single-concern diffs directly.
- Reviewers must come from a different model provider than the author:
  Claude-written changes are reviewed by Codex, and Codex-written changes by
  Claude. Pick the most capable model available from that provider. In T3,
  look up the current models with `orchestrator_capabilities` and start the
  reviewer with `delegate_task`; native subagent tools usually can only start
  agents from the same provider. If no other provider is available, say so in
  the report. Only then fall back to a fresh agent from the same provider, and
  label its review as a weaker substitute, not an independent one.
- Give each reviewer a self-contained brief: the base revision and exact diff
  range, the intended behavior, the docs and modules that matter, and the
  "Code review" rules above. Don't pass along the author's own assessment as
  fact. Reviewers should test the claims, not inherit them.
- Split the work by risk area, not by file. Examples: XMP and settings
  compatibility, session and history consistency, job inputs and cache
  identity, worker lifecycle and shutdown, save before navigation, rendering
  parity, and architecture boundaries. Only start an area the diff actually
  touches, and tell each reviewer to follow callers and error paths outside
  the diff.
- Reviewers only read. They don't edit, commit, push, use `git stash`, or
  re-bless references. If a reviewer has to build or run code, give it its own
  worktree. Running several Cargo builds in one checkout makes them wait on
  each other's lock. One agent runs the CI checks. Dataset and Camera Raw
  parity runs follow `tests/corpus/README.md` and never run at the same time
  as each other.
- Ask each reviewer to report findings in a fixed structure: severity,
  file:line, triggering scenario, consequence, evidence or a reproduction,
  and whether it's confirmed, a potential risk or pre-existing. A reviewer
  with nothing to report should say so and state what it covered.
- Subagent findings are claims, not results. Before reporting a finding, the
  orchestrator re-reads the cited code and checks that the triggering scenario
  actually holds. Merge duplicates, keep design preferences out of the defect
  list, and list every rejected finding in the report with the concrete reason
  it was rejected.
- For follow-up rounds, start a new task each time. Give it the original brief,
  earlier findings, the responses to them and any open objections, so settled
  points aren't argued again.
- The final report follows the "Code review" format. It also lists which risk
  areas were reviewed, which were skipped and why, any reviewer that failed or
  timed out, and any checks that couldn't run here, such as ones that need the
  private corpus.
- Don't push, open or update a pull request, or merge until the reviewer has
  approved the latest revision. Approval means no unresolved findings: each is
  either fixed or declined with a concrete reason. After any fix, ask for
  another round on the new revision; an earlier approval doesn't cover later
  changes. When the author and reviewer disagree about whether a finding is
  worth fixing or just a nitpick, the user who requested the change decides.
- Docs-only commits and release commits skip review and local checks; see
  "Before every push".

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

Run `make check` and push only when it passes. It runs every check CI runs
(`.github/workflows/ci.yml`) that can run on this machine: dependency and crate
closure checks, the Python calibration tests, formatting, clippy, every
crate's tests and the MSRV build. The Makefile lists what only CI covers (the
other platforms, the Arch package, Postgres). Use the current stable Rust, as
CI does; newer clippy releases add lints. Where rustup's `cargo +toolchain`
proxy doesn't work, set `MSRV_BIN` to the MSRV toolchain's `bin` folder.

A `v*` tag builds and publishes a release, so the same applies before tagging.

Docs-only commits and `Release x.y.z` commits that change no code are the
exception: push them straight to `main` without a pull request, subagent
review or local checks, and don't wait for CI. Add `[skip ci]` to docs-only
commit subjects, but never to a release commit: GitHub applies it to the tag
push too, which would skip the release workflow.

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
2. Write and review the notes, and commit as `Release x.y.z`, with a short
   summary of what changed. Run the required checks only if the commit changes
   code.
3. Push `main`, verify the release commit is on `origin/main`, then create
   and push its annotated tag `vx.y.z`.
4. Wait for every build, signing/notarization, installation check and publication.
   Upload any linked media and verify the published text, images and downloads.
   A pushed tag alone does not complete a release. GitHub publication is enabled;
   AUR publication remains disabled.

## Commits

- Commit only the files you changed, by explicit path.
- Never commit Adobe profiles, RAW files or paths from your own machine.
