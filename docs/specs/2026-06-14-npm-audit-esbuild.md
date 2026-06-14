# NPM Audit Esbuild Remediation

## Context

`npm audit` reported high-severity findings through Vite's transitive `esbuild`
dependency. The vulnerable range was `esbuild >=0.17.0 <0.28.1`, and the
current Vite stack resolved `esbuild@0.25.12`.

## Decision

Wardian keeps the existing Vite 6 and VitePress 1 stack, upgrades Tailwind's
Vite integration to the current compatible patch line, and overrides `esbuild`
to `0.28.1`.

Vite 8 was evaluated but is not a clean replacement for this PR because
VitePress 1.6.4 still depends on Vite APIs that changed under Vite 8. A full
VitePress 2 migration should be handled separately.

`esbuild@0.28.1` also removed support for lowering destructuring to Vite's
legacy default browser target. Wardian and its documentation builds now target
ES2022 explicitly, which matches the modern WebView/browser baseline used by
the app and avoids the incompatible transform path.

## Verification

The remediation is validated by `npm audit`, `npm run build`, and
`npm run docs:build`.
