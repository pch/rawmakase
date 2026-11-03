---
name: release
description: Cut a RAWmakase release x.y.z, optionally after a given PR merges. Use for "cut a new release", "when #N merges, release 0.4.x", "follow release instructions", "credit contributors", "do a tldr", retrying a failed release build, or asking what would go into the next release.
---

# Cut a release

Policy lives in `AGENTS.md` ("Releases", "Before every push") and
`packaging/RELEASING.md`. Read both before starting. This skill is the order of
operations and the recovery steps that have come up before.

## Inputs

Read these from the request:

- **version** `x.y.z`, required. Check it's greater than the latest `v*` tag.
- **after**: PR numbers that must merge first. Watch them and fix CI failures
  on them if asked ("make sure they merge").
- **highlights**: what the user wants to lead with, plus anything they want
  spelled out: limits, licensing reasons, "experimental", topics to leave out.
- **fingerprints**: the dataset decode-fingerprint check ("no need to
  fingerprint" skips it).
- **skip**: jobs the user said not to wait for, or platforms to drop from this
  release. Only drop a platform when the user says so.

## 1. Wait for the gating PRs

If `after` names PRs, call `watch_pull_request` on each and end the turn until
they're merged. Then `git fetch origin` and work from `origin/main` in this
worktree. Never release from a stale or local-only `main`.

"What would go into the next release?" stops here: list the merged PRs and
issues since the last tag, grouped the way the notes would be, and the
contributors.

## 2. Gather the actual range

```sh
prev=$(git describe --tags --abbrev=0 --match 'v*' origin/main)
git log --first-parent --oneline "$prev"..origin/main
gh pr list --state merged --base main --search "merged:>=$(git log -1 --format=%cI "$prev")" \
  --json number,title,author,closingIssuesReferences --limit 200
```

- Read the diffs and PR/issue context for anything user-visible. Leave out
  reverted, partial or flag-hidden work.
- Contributors: PR authors other than `pch` and bots, `Co-authored-by` trailers
  and PRs that were superseded but credited ("isolated fix" PRs). Reporters:
  the authors of the issues these PRs close, and discussions they cite. Don't
  credit anyone the history doesn't support.

## 3. Write the release commit

1. Bump the version in `Cargo.toml`, `Cargo.lock` (the `rawmakase` package
   entry only), `packaging/macos/Info.plist` (`CFBundleShortVersionString`)
   and both `packaging/arch/*/PKGBUILD` (`pkgver`). Compare against the
   previous `Release x.y.z` commit: `git show <prev release commit> --stat`.
2. Write `packaging/release-notes/vx.y.z.md` following "Writing release notes"
   in `RELEASING.md`. Use the latest notes for structure, not for claims:
   - A summary paragraph leading with the user's highlights.
   - Only the sections that apply: **New**, **Improved**, **Fixed**,
     **Changed**, **Removed**. Each bullet starts with a bold user-visible
     result and links the PRs and issues.
   - **Upgrade notes** for catalog or XMP migrations, saved edits that render
     differently, or changed defaults. **Known limitations** where the user
     asked for limits spelled out.
   - Thanks naming contributors and reporters, with links.
   - **Downloads** with the exact asset names from `RELEASING.md`, and the
     full-changelog compare link `vprev...vx.y.z`.
   - Measured numbers only with the conditions they were measured under.
3. Check every claim against the code at `origin/main`. Then show the user
   the notes and a short TL;DR, and wait for approval unless they said to go
   ahead.
4. Fingerprints: unless skipped, follow `RELEASING.md` step 2 on a release
   build if `$RAWMAKASE_DATASET` exists here. If it doesn't, say it was
   skipped and why.
5. Commit only those files as `Release x.y.z`. The body is one paragraph of
   plain-language highlights, like the previous release commit. **No
   `[skip ci]`**: it would also skip the tag's release workflow. No PR,
   subagent review or local checks, since the commit changes no code (if it
   does, run the checks).

## 4. Push and tag

```sh
git push origin HEAD:main
git fetch origin && test "$(git rev-parse origin/main)" = "$(git rev-parse HEAD)"
git tag -a vx.y.z -m "RAWmakase x.y.z" && git push origin vx.y.z
```

If `main` moved meanwhile, rebase the release commit, check that the notes
still cover the range, and push again. Never force-push `main`.

## 5. See it through

The release isn't done when the tag is pushed.

- Find the run (`gh run list --workflow release.yml --limit 3`) and follow it
  with `gh run watch <id> --exit-status` in the background. Don't poll in the
  foreground.
- Publishing waits for maintainer approval of the `release-signing`
  environment. Once all the checks before it pass, send the user the run URL
  and ask them to approve.
- After publishing, check that **Publish Homebrew cask** succeeded and the tap
  has the new version.
- Verify the published release: notes text, every download link (`curl -sIL`
  on each asset URL returns 200), `SHA256SUMS`, and any media.

Recovering from failures (the details are in `RELEASING.md`):

- **macOS signing or notarization** fails, often on Intel: rerun only the
  failed jobs (`gh run rerun <id> --failed`), which repeats signing, not the
  build.
- **Packaging needs a code fix**: fix it on `main` through `ship`, then run
  **Release** from `main` with the existing tag and `publish` on
  (`gh workflow run release.yml --ref main -f tag=vx.y.z`). Don't move the
  tag. Only retag if the user asks and nothing has been published yet.
- **Notes-only correction**: commit the notes file to `main` with
  `[skip ci]`, then `gh release edit vx.y.z --notes-file packaging/release-notes/vx.y.z.md`.
- **A job takes too long**: ask before cancelling or dropping anything.

## 6. After publishing

- If the user asked, or `.github/ISSUE_TEMPLATE/` names versions this release
  makes stale, update the template directly on `main` (docs-only, `[skip ci]`).
- Report: release URL, tag SHA, platforms published, anything skipped
  (fingerprints, jobs, platforms), and the TL;DR.
