# Pull request delivery

## Scope and authority

For implementation tasks in the Wardian repository, the default deliverable is
a committed, pushed branch and an open, issue-linked pull request whose
applicable hosted CI checks have finished successfully on the latest published
commit. Publication is an intermediate step; CI follow-through is part of the
task. The
repository's AGENTS.md grants standing authorization: do not stop after local
implementation to ask whether to publish. An explicit local-only request
overrides this default. Read-only investigation, explanation, or review does
not authorize implementation or publication.

This policy applies to Wardian only. Keep shared skills and class/global
instructions project-neutral. CLAUDE.md and GEMINI.md already import AGENTS.md;
do not duplicate the policy in those files.

## Publish verified work

1. Inspect the branch, base, and complete diff. Preserve unrelated work and
   keep one issue's changes per PR. Reuse the matching tracking issue, or
   create one when none exists; this is part of the authorized publication.
2. Run [local CI verification](./ci-verification.md). Narrow reruns to the
   affected category when appropriate. Resolve failures or establish an exact
   base reproduction before recording a limitation; never report an unrun
   check as passing.
3. Use Wardian's `autoreview` workflow when it is available to obtain an
   independent local-agent verdict; otherwise obtain that verdict through a
   reviewer agent. Address blocking findings and record the verdict and
   evidence. Review ends at zero blocking findings; track non-blocking
   follow-ups in a linked issue. If a structured reply is unavailable, inspect
   the reviewer's conversation log for the verdict rather than inferring
   approval from idle status.
4. Commit only the intended files with a semantic message, then push the task
   branch. Do not force-push or overwrite another contributor's work.
5. Open a PR or update the existing PR for that branch. Use the repository
   template, link its issue, and include verification and local-review evidence.
   Use a body file for multiline text. Follow
   [screenshot documentation](./screenshot-documentation.md) for UI changes.
6. Verify the published head, issue link, rendered body, and mergeability.
   Monitor all applicable hosted CI checks through completion on that exact
   commit. Investigate failures, make the necessary corrections, repeat affected
   local verification and review, then push and monitor the new commit. An older
   commit's green checks do not validate a newer commit.
7. Finish only after all applicable checks have completed successfully and the
   repository's four readiness conditions hold. Record intentional skips and
   their reasons separately; do not count them as passes. Pending, queued,
   running, cancelled, and failed checks leave the task incomplete. Keep
   monitoring and fixing without requiring another user prompt. A progress
   update containing a PR URL is not a completed delivery.

If an external blocker prevents a passing result, report the task as blocked
with the affected commit, check, observed cause, and next action. Do not describe
it as done or ready, weaken checks, or infer success from a local pass. Hosted CI
completion does not grant merge or deployment authority.

## Review and publication boundaries

Zero-blocker review means review by local agents, not GitHub approval. Never
request reviewers on GitHub: do not use reviewer flags, review-request API
calls, or the GitHub reviewer UI. GitHub's `reviewDecision` is not a substitute
for the local-agent verdict and is not a reason to solicit GitHub reviews.

Do not change branch protection to clear a hosted review requirement. This
policy authorizes publication, not merging, deployment, or unrelated cleanup.
If publication cannot proceed because of credentials or another external
blocker, report that blocker and the completed local evidence explicitly.
