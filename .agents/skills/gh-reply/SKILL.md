---
name: gh-reply
description: Write and post GitHub comments on RAWmakase issues, PRs and discussions, signed by the AI model that wrote them, on the maintainer's behalf. Use for "reply to the issue", "comment as claude/codex", "sign it", "thank the contributor", "ask the reporter", "refuse/close this PR", "let me approve your reply before you post", or any comment, review reply or closing note on GitHub.
---

# Reply on GitHub on the maintainer's behalf

Comments are posted from the maintainer's account (`pch`) with `gh`, so every
one must say plainly which AI model wrote it. Posting is public and gets emailed
to subscribers. Editing afterwards doesn't take it back, so get it right
first.

## Rules

- **Always sign as yourself.** End every comment with a signature naming the
  model that wrote the text, on its own line after a blank line:

  ```
  — <product> <model> (on behalf of @pch)
  ```

  For example `— Claude Opus 5.5 (on behalf of @pch)` or
  `— Codex GPT-6-Astra (on behalf of @pch)`. Use the model you are actually
  running as, from your own environment, not a name from this file, an earlier
  comment or the user's phrasing ("as claude" means "sign it", not "sign as
  Claude" if you're another model). If you don't know your exact model, use
  the product name alone. If a subagent from another provider wrote the text,
  sign with that model. This includes short thank-yous, edits and closing notes. If a comment went
  out unsigned, or signed with the wrong model, edit it to fix the signature
  (`gh api -X PATCH repos/pch/rawmakase/issues/comments/<id> -F body=@/tmp/reply.md`).
  Don't post a second comment.
- **Draft or post.** Post directly when the user said "post", "reply" or
  "comment" and the content is settled. Show the draft and wait when the user
  said "let me approve", or when the reply makes a decision they haven't made:
  refusing or closing, a promise about future work, a statement on licensing,
  or anything about another project. When unsure, draft.
- **Only what's true and public.** Every claim about the code, a release or a
  fix must be checked against `origin/main`, the PR diff or the release. Say
  "fixed in 0.4.12" only once it has shipped. Before that, say "fixed on main,
  in the next release". Don't mention private datasets, Adobe profiles, local
  paths, other users' photos, or context the user asked you to leave out.
- **Write for the reader, not for us.** Plain words, no internal module
  names, short paragraphs. Link PRs, issues and docs instead of explaining
  them again. Match the reporter's language level.
- `@` notifies people. Only use it for people meant to be pinged. Put code and
  placeholders in backticks.

## Posting

```sh
gh issue comment <n> --body-file /tmp/reply.md        # issue
gh pr comment <n> --body-file /tmp/reply.md           # PR conversation
gh pr review <n> --comment --body-file /tmp/reply.md  # review summary
gh api repos/pch/rawmakase/pulls/<n>/comments/<id>/replies -F body=@/tmp/reply.md  # inline thread
gh pr close <n> --comment "$(cat /tmp/reply.md)"      # close with a note
```

Always use a body file. Shell quoting breaks Markdown and backticks. After
posting, fetch the comment back and give the user its URL.

## Kinds of reply

**Asking a reporter for details.** Assume they may not be technical.

- Start by thanking them for one specific thing they did, then say what you
  think is happening in one or two sentences.
- Give a workaround, if there is one, as numbered steps with exact menu paths
  and the button labels they will see.
- Ask numbered questions. Each should be answerable in one line, and say where
  to find the answer. Ask only for what changes the next step.
- For system details, ask for the system information instead of single facts:
  "click the bug icon at the bottom of the left sidebar (or Preferences ▸
  General ▸ Report an Issue…, Help ▸ Report an Issue… on macOS), then Copy
  System Info and paste it here". On 0.4.5 or earlier, ask for the
  installation method, OS version and CPU instead.
- If a build or log would help, say exactly how to get it and what to paste.

**Investigation or status update.** What we found, what's fixed and where
(PR, release), what's still open and why, and what would help next. Keep the
issue open unless it's fixed or the user decided to close it.

**Thanking and merging a contributor's PR.** Name what the change does well,
specifically (the test that catches the edge case, the clear notes), mention
what you checked, and say it's merging. For a first contribution, say so
warmly. If follow-up work was split out, link it.

**Declining a PR or part of one.**

- Start with thanks, and with what's useful in it.
- Give the concrete reason, without hedging. For legal reasons (code copied
  from or derived from GPL or proprietary sources, reverse-engineered Adobe
  data, patent-encumbered methods), name the kind of risk. Don't accuse anyone
  or give legal advice.
- Offer a way forward: what an acceptable implementation would look like, or
  that a narrower PR is welcome.
- If we took part of the work into our own PR, link it and say the
  contributor is credited there (`Co-authored-by` in the commit, a mention in
  the release notes).
- Close the PR only if the user said to.

**Feature requests and "will you support X".** Say whether it's planned, the
real constraint (decoder support, licensing, missing hardware to test on), and
whether a contribution would be welcome. Don't commit to dates.

## After posting

Report the comment URL, whether it was a draft you approved or posted
directly, and any follow-up it promises (a fix, a build, a check after the
reporter answers). If the thread is being watched, say what reply you're
waiting for.
