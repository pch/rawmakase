---
name: ship
description: Take finished work on a branch through cross-provider review, a pull request, CI and merge. Use when asked to "let codex review", "review then open a PR", "merge when green", "repeat until you get an ok", or to plan a change with a reviewer until both agree. Also covers re-review after bot comments, rebases and conflicts on a watched PR.
---

# Ship a change

Review → PR → CI → merge, following the "Code review with subagents", "Before
every push" and "Commits" sections of `AGENTS.md`. This skill is the order of
operations; `AGENTS.md` is the policy. Where they disagree, `AGENTS.md` wins.

## Options

Read these from the request. Defaults in brackets.

- **reviewer**: the other provider's model [Claude-authored → Codex
  `gpt-6-astra`, `reasoningEffort: medium`; Codex-authored → Claude
  `claude-opus-5-5`]. "codex astra medium", "sol 6.1", "sonnet" etc. name the
  model and effort. Check the IDs with `orchestrator_capabilities`.
- **merge**: merge when green and approved [yes, unless the user said "open a
  PR" only, or "let me test first"].
- **skip slow CI**: don't wait on named jobs, usually "Test on Windows" [no].
- **plan only**: review a plan instead of code; stop for user approval before
  implementing (see the end of this file).

## 1. Get the branch ready

- Work in a T3 worktree on a topic branch, never on `main`. If changes landed
  in the main checkout or on `main`, move them to a branch first.
- Make the commits tell the story: squash fixups. Commit by explicit path.
  Never commit Adobe profiles, RAW files or machine paths.
- Run the CI checks from `AGENTS.md` ("Before every push") plus any relevant
  dataset, dependency or rendering check. Note anything you could not run.
- Docs-only changes skip this skill: they go straight to `main` with
  `[skip ci]`.

## 2. Review loop

Each round is a new `delegate_task` call. Never continue a review with
`t3_thread_send` on the child thread.

- `clientRequestId`: `<topic>-review-r<N>-<short head sha>`, stable across
  retries of the same round. Keep the returned `taskId`.
- Give the reviewer its own worktree (`isolation: "worktree"`) if it needs to
  build or run code. Commit first: uncommitted edits are not copied.
- Large diffs touching several risk areas: one reviewer per area that the diff
  actually touches (see `AGENTS.md`), at most about four.

The brief must be self-contained:

1. Base SHA and exact range `base..head`, the branch, and the issue or PR URL.
2. Intended behavior and constraints, in the user's terms. Don't include your
   own claim that it is correct.
3. Docs to read: `docs/architecture.md`, `docs/code-map.md`, the relevant
   module docs and `tests/corpus/README.md` if rendering is involved.
4. "Follow the Code review section of AGENTS.md. Read only: no edits, commits,
   pushes, `git stash` or reference re-blessing. Follow callers and error paths
   outside the diff."
5. Report format: severity, file:line, triggering scenario, consequence,
   evidence or reproduction, confirmed / potential risk / pre-existing. If
   there are no findings, say so, state the coverage and give an explicit
   verdict on the head SHA.
6. From round 2: earlier findings, your response to each (fixed in which
   commit, or declined and why) and any open objections. Ask the reviewer to
   check the fixes and not reopen settled points.

After each round:

- Treat findings as claims. Re-read the code and check the scenario before
  acting. Fix real defects with a regression test. Decline the rest with a
  concrete reason.
- Approval means no unresolved findings **on the current head SHA**. Any code
  change after approval needs another round. Pure rebases without conflict
  resolution and merge commits from `main` don't need one.
- If the reviewer and you still disagree after two rounds on the same point,
  ask the user.

If the reviewer fails:

- Model at capacity: retry with the next-strongest model from the same
  provider.
- Billing, quota or provider outage: switch to another non-author provider
  (Grok). If none is left, say so and only then use a same-provider fresh
  agent, labelled as a weaker substitute.
- Worktree provisioning failed: retry once. If the review doesn't need a
  build, rerun without isolation. Otherwise report it.
- Timed out or no verdict: start a new round with the same brief. Don't count
  it as approval.

## 3. Pull request

- Push and open the PR with `gh pr create` against `main` (or the stacked
  parent branch). Call `link_pull_request` right away.
- Body: what changes for the user and why, how it was tested, skipped coverage,
  and a short note of the review (rounds, reviewer model, declined findings and
  why). End with the attribution line from the system reminder.
- Credit external contributors and reporters with `Co-authored-by` or in the
  body when the work builds on theirs.

## 4. Watch, respond and merge

- Call `watch_pull_request` and end the turn. Don't poll.
- Bot reviews (Codex connector, CodeRabbit) and human comments follow
  "Addressing code review comments" in `AGENTS.md`. Commit a code fix locally
  and get it approved in another review round **before** pushing it. Pushing
  updates the PR. Reply on the thread when you decline something, following
  `gh-reply`.
- Conflicts with `main`: rebase. If resolving the conflicts changed code, run a
  review round on the resolution before force-pushing.
- CI failure: read the log (`gh run view --log-failed`), fix it, and treat the
  fix like any other code change: local checks, review round, then push.
- Merge when the latest head has reviewer approval and every check is green,
  except jobs the user said to skip. The repo uses merge commits:
  `gh pr merge <n> --merge`. Auto-merge is disabled, so merging is up to you.
- After merging, check that the commit is on `origin/main`, call
  `unwatch_pull_request` and report: PR URL, merge SHA, review rounds and
  anything skipped.

## Plan only

Used for "plan with codex until you both agree, let me approve":

1. Write the plan: goal, user-visible behavior, design, affected modules and
   docs, XMP/settings compatibility, tests, risks, open questions.
2. Run the same review loop on the plan, with no code range. The reviewer may
   read code and, if the user allowed it, use the computer (Lightroom, Camera
   Raw) to measure.
3. When there are no unresolved objections, show the user the plan, the
   points you disagreed on and how each was settled. Don't implement until the
   user approves.
