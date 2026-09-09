# Site Media Capture

The feature site at `wardian.org` is built around short looping clips of the
real Wardian UI. This page covers producing them.

The clips are recorded by a script in this repository rather than copied by
hand, so they can be regenerated on every release. Before this existed the site
drifted four releases behind, because refreshing it meant a person moving files
between two repositories.

## Running the capture

```sh
npm run site:media
```

The script starts its own Vite dev server on port 1431 with an isolated
`WARDIAN_HOME`, records each clip with Playwright, transcodes them with ffmpeg,
and writes everything to `docs/assets/site-media/`.

**ffmpeg must be on `PATH`.** The script checks for both `ffmpeg` and `ffprobe`
before it records anything and stops with an install pointer if either is
missing.

PowerShell (Windows):

```powershell
npm run site:media
```

Two environment variables override the defaults:

| Variable | Effect |
| --- | --- |
| `WARDIAN_SITE_MEDIA_PORT` | Port for the owned dev server. Default `1431`. |
| `WARDIAN_SITE_MEDIA_URL` | Capture an app already running at this URL instead of starting one. |

The stills capture (`npm run docs:screenshots`) defaults to port 1420, so the
two can run back to back without colliding.

## What it produces

```
docs/assets/site-media/
  manifest.json
  <clip-id>.mp4      H.264, yuv420p, +faststart, no audio
  <clip-id>.webm     VP9, no audio
  <clip-id>.png      poster frame
```

`manifest.json` is the integration seam with the site repository:

```json
{
  "generated_at": "2026-09-05T23:00:00.000Z",
  "app_version": "0.6.0",
  "clips": [
    {
      "id": "graph",
      "mp4": "graph.mp4",
      "webm": "graph.webm",
      "poster": "graph.png",
      "width": 1600,
      "height": 1000,
      "duration_ms": 8200,
      "bytes_mp4": 512345
    }
  ]
}
```

The site consumes the same filenames, flat, from its own `assets/media/`.

## Constraints the script enforces

These are checked in the run, not left to review:

- **Every clip is 6-12 seconds.** A clip outside that window fails the run and
  names itself, so the choreography gets adjusted rather than the budget.
- **Every mp4 is under 900 KB.** An oversized clip should be shortened, not
  shipped.
- **Every captured id appears in `EXPECTED_CLIPS`.** A clip added to the
  sequence but not to the manifest list would never be reported as skipped.
- **A run that fails partway names the clips it never reached.** Those files are
  still on disk from the previous run and still look current. This is the same
  staleness discipline the stills capture uses, and it exists because a stale
  screenshot once survived several releases unnoticed.

Required clips fail the run when they cannot be produced. Stretch clips are
reported and skipped, because an empty or broken clip is worse than an absent
section.

The distinction tracks the site, not the difficulty of the capture: a clip is
stretch only while no section on wardian.org uses it. Once a section does, a
skipped capture would publish that section with no media, so the clip moves to
`REQUIRED_CLIPS`. There are no stretch clips at present.

## Shared fixtures

Both captures drive the same seeded app through
[`scripts/lib/docs-app-mock.mjs`](https://github.com/wardian-app/Wardian/blob/main/scripts/lib/docs-app-mock.mjs),
which holds the in-browser Tauri IPC mock and every fixture. They live in one
place so the two captures cannot show different data for the same surface.

The fixtures are deterministic and public-safe: paths are the
`<absolute-workspace-path>` and `<wardian-home>` placeholders and the agents are
invented. Never point a capture at a real workspace — the repository and the
site are both public.

The Explorer re-roots on selection the way the app does: an agent's workspace
when one is selected, the Wardian home when none is. A clip that needs to reach
Wardian's own files, rather than a project's, must therefore leave the roster
selection empty.

One difference between the two captures matters. The stills apply
`stabilizeVisuals()`, which zeroes animation and transition durations so the
images are byte-stable. The video capture deliberately does not, because it
would flatten every clip into a still image.

## Adding a clip

1. Add the id to `REQUIRED_CLIPS` (or `STRETCH_CLIPS`) in
   `scripts/capture-site-media.mjs`.
2. Add an entry to `CLIPS` with that id and a `run(page)` choreography. Aim for
   roughly 8 seconds of deliberate movement; the recorded app boot is trimmed
   automatically, so time only the actions.
3. If the surface needs seed data the shared mock does not have, pass it through
   the mock's options rather than editing fixtures other clips rely on:
   - `fixtures` shallow-overrides any exported fixture.
   - `commandResults` answers IPC commands the mock has no handler for, as plain
     serializable values.
4. Run `npm run site:media` and check the clip frame by frame for leaked paths
   before committing.

A clip whose surface the mock cannot drive should be left out and said so
plainly. Do not substitute an easier surface to fill the slot.

## Refresh workflow

[`.github/workflows/site-media.yml`](https://github.com/wardian-app/Wardian/blob/main/.github/workflows/site-media.yml)
runs the capture on every published release and on manual dispatch. It always
uploads the result as a `site-media` workflow artifact.

### The one manual setup step

Opening the pull request against the site repository needs its own credential,
because `GITHUB_TOKEN` cannot write to another repository.

It comes from the organization's existing release-dispatch GitHub App, the one
[`release.yml`](https://github.com/wardian-app/Wardian/blob/main/.github/workflows/release.yml)
already uses to reach `homebrew-tap` and `packages`. There is no new secret to
create: `WARDIAN_RELEASE_DISPATCH_APP_ID` and
`WARDIAN_RELEASE_DISPATCH_PRIVATE_KEY` are configured, and this workflow mints
a token from them with `actions/create-github-app-token`.

Two things have to be true of that App, both set in its settings rather than
here:

1. It is installed on `wardian-app/wardian.org`. It was installed for release
   dispatch, so its repository access probably lists only `homebrew-tap` and
   `packages`.
2. It holds **Contents: write** and **Pull requests: write**. Release dispatch
   only needs Actions: write, so these are likely additional permissions, and
   adding them raises a request an organization owner has to approve on the
   installation.

The token this mints is scoped to `wardian.org` and those two permissions, and
expires within the hour. Nothing long-lived is stored, and nothing is tied to
one person's account — which is why this is worth the two settings changes over
a personal access token.

Until both are true the workflow still captures the clips and uploads the
artifact, and logs a warning explaining what is missing. A failure to mint the
token is treated the same way, because an App that is not installed yet is a
setup state rather than a broken run. The bundle can be downloaded and copied
by hand in the meantime, so the pipeline is useful before the App is set up.

### What the refresh may delete

`assets/media` in the site repository holds more than this pipeline produces:
the remote-control phone stills are committed there directly. The copy step
therefore clears videos and the posters that sit beside them, never a `.png`
on its own. Keep that rule if you change the step, or a refresh will silently
delete images the homepage still references.
