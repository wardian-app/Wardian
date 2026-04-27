# Release Artifact Ownership

## Context

Wardian `v0.3.1` was published by Release Please without executable release assets. The Tauri artifact workflow did not run because Release Please created the tag and GitHub release with the default `GITHUB_TOKEN`; GitHub does not trigger normal downstream workflows from most `GITHUB_TOKEN`-initiated events.

## Decision

Release Please owns version calculation, changelog updates, release PRs, and tag creation. The `Release` workflow owns Tauri artifact upload and final publication.

To enforce that split without requiring a personal access token, `release-please-config.json` creates GitHub releases as drafts. When Release Please reports a created release, `.github/workflows/release-please.yml` dispatches `.github/workflows/release.yml` through `workflow_dispatch`, passing the tag as both the build ref and the existing release tag. The release workflow resolves that draft release, uploads platform bundles through `tauri-apps/tauri-action`, and publishes the release only after the build matrix succeeds.

Manual `workflow_dispatch` runs may also backfill assets onto an existing release. In that mode, the workflow checks out the requested `tag`, resolves the existing GitHub release from `release_tag`, and uploads artifacts to that release ID. Dry runs still build and retain Actions artifacts without mutating a GitHub release.

## Verification

`src/config/releaseWorkflow.test.ts` guards the contract:

- Release Please-created releases must remain draft until assets are uploaded.
- Release Please must dispatch the artifact workflow when it creates a release.
- The release workflow must remain tag-triggered for manually pushed tags.
- The release workflow must still upload Tauri artifacts to a release ID before publishing.
- Manual backfills must resolve an existing release and upload to that release ID.

This prevents a future release from silently returning to the broken flow that produced an assetless public release.
