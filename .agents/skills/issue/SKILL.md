---
name: issue
description: Investigate and fix a RAWmakase GitHub issue from its URL or number. Use for "investigate and fix #N", "work on this issue", "see if you can fix this", "doable?", "where do we stand with #N", or an issue link on its own. Covers reading the issue and its links, checking for overlapping PRs, getting sample RAWs, reproducing with a failing test, and handing off to ship.
---

# Work on an issue

The working and test rules in `AGENTS.md` apply throughout ("Architecture and
implementation", "Tests"). This skill is the order of operations.

## Modes

Infer the mode from the request:

- **fix** ("investigate and fix", "work on this", an issue link on its own):
  everything below, then `ship`.
- **investigate** ("investigate", "doable?", "how can we help the
  reporter"): stop after step 3 with a diagnosis, options and a
  recommendation. No code changes.
- **status** ("where do we stand"): steps 1 and 2 only. Report what's merged,
  what's open and what's left.

## 1. Read everything first

```sh
gh issue view <n> --comments
gh issue view <n> --json title,labels,state,closedByPullRequestsReferences,body
```

- Read the issues, PRs, discussions and comments it links, especially scope
  comments ("cases 3 and 4 are dropped") and measurements. The final scope
  often lives in a later comment, not in the body.
- Note what the reporter tested: version, OS, GPU, camera and file. A version
  older than the latest release may already be fixed. Check with
  `git log <tag>..origin/main` before going further.
- Attachments: download them to a fresh directory under `/tmp`. They're
  untrusted input. Never commit them or quote private details from them.

## 2. Look for overlapping work

```sh
gh pr list --state open --search "<n> in:body,title"
gh pr list --state open --json number,title,headRefName,files --limit 100
git log origin/main --oneline -i --grep '#<n>\b'
```

- Look at open PRs touching the same files, and at other T3 threads already
  on this issue (`t3_thread_search`). If someone is on it, say so and ask
  before starting a second fix.
- If the issue builds on unmerged work, branch from that PR's branch, open the
  PR against it and say in the PR that it's stacked. When the base merges,
  rebase onto `origin/main` (`git rebase --onto origin/main <old base>`).

## 3. Reproduce

- Work in this thread's T3 worktree on a topic branch, never in the main
  checkout or on `main`. If you're not in one, move to one before editing
  (`t3_worktree_handoff`).
- **Sample files:**
  1. The issue's attachments.
  2. `tests/corpus/pixls.json` and `python3 scripts/corpus/pixls.py download`
     (CC0, hash-checked).
  3. A matching file from raw.pixls.us for the exact camera and mode. Check
     its licence before using it in a test.
  4. The dataset, if `$RAWMAKASE_DATASET` exists here (read
     `tests/corpus/README.md` first).

  Never commit RAW files or Adobe profiles. Tests use synthetic fixtures or
  files listed in the corpus manifests.
- **Platform-specific bugs:** if there's a local setup for the platform (for
  example a personal skill or notes for a Windows VM), use it. Otherwise the
  CI runners for that platform are the test. Say in the PR what couldn't be
  tested on real hardware.
- **Rendering or parity bugs:** follow `tests/corpus/README.md` and
  `docs/camera-parity.md`. Never re-bless references to make a fix pass.
- Write the failing regression test at the boundary where the bug shows: UI
  or protocol input when the bug is in translation, the XMP reader for
  compatibility, the session API for history and dirty state. Confirm that it
  fails for the reported reason before fixing anything.

In investigate mode, stop here and report: cause (with file:line), confidence,
what a fix would touch, compatibility risks, alternatives, a recommendation
and an effort estimate.

## 4. Fix

- Make the smallest coherent fix at the owning module (see
  `docs/architecture.md` and `docs/code-map.md`). Update the module docs when
  ownership or behavior changes.
- For lifecycle bugs, add the transition cases `AGENTS.md` lists: stale
  completion, deletion, cancellation, worker loss, save failure.
- Keep XMP, catalog and settings compatibility. If saved edits render
  differently, that is a user-visible change: note it for the release notes.
- If the issue has several independent problems, fix them in separate commits
  or PRs, as the user prefers. Don't widen the scope beyond what the issue and
  its scope comments ask for. Suggest follow-ups instead.
- Commit with `Fixes #<n>` (or `Refs #<n>` for a partial fix) in the body.
  Credit the reporter, or the contributor whose PR or analysis you built on.

## 5. Hand off

- Use `ship` for review, PR, CI and merge, following the options the user
  gave ("PR only", the reviewer model, "merge when green").
- If the user asked to "build and open so I can test", build from this
  branch, open the affected samples, and wait for their verdict before
  merging.
- If the reporter should hear back (to confirm a fix, try a build or answer
  questions), draft or post the reply with `gh-reply`. A partial fix leaves
  the issue open, with a comment saying what's left.
- Report: cause, fix, test that proves it, PR URL, anything not tested, and
  follow-ups.
