# macOS Notarized Release Distribution

## Status

Accepted for the desktop release workflow.

## Context

Wardian distributes a macOS app outside the Mac App Store through GitHub
Releases and Homebrew Cask. A macOS app launched directly from a mounted DMG is
on a read-only volume, so the in-app updater cannot replace it. Local source
builds also do not carry a public release identity by default. These cases can
look similar to users, but they are not evidence that an official release
artifact is invalid.

## Decision

Each release workflow macOS build must:

1. import a password-protected Developer ID Application certificate into an
   ephemeral runner keychain;
2. sign the application and nested code using that identity;
3. submit the release artifacts to Apple notarization and staple the resulting
   ticket;
4. fail before publication if any Apple signing or notarization secret is
   missing; and
5. verify the final DMG and updater archive with `codesign`, `stapler`, and
   Gatekeeper assessment.

The Homebrew tap independently downloads and verifies both published DMGs
before it updates the cask. A matching SHA-256 checksum alone is not treated as
sufficient evidence that a macOS artifact is ready for this distribution path.

## Operational Rules

- Keep the Developer ID certificate, P12 export password, Apple app-specific
  password, and Tauri updater signing key as distinct secrets.
- Use an Apple app-specific password for notarization, never the normal Apple
  ID password.
- Treat source builds and historical unsigned builds as separate from official
  release artifacts. They may need explicit local trust approval and are not a
  valid notarization test.
- Document the supported user install path as: open the DMG, copy Wardian to
  `/Applications`, eject the DMG, then launch the installed copy.
- Treat OS error 30 during an update as an install-location diagnosis first:
  a mounted DMG is read-only. Reinstall into `/Applications` before blaming the
  release artifact.

## Verification and Rollout

Run the release workflow in dry-run mode on GitHub Actions after configuring
the Apple secrets. It must complete the macOS artifact verification step before
the draft release can publish. Then perform one clean-macOS install and update
test using the copy in `/Applications`.

The tap update workflow must pass its independent DMG validation and Homebrew
audit before its cask pull request can be merged.
