# Claude managed instruction projection

- **Status:** Implemented; real-provider acceptance pending
- **Date:** 2026-09-07
- **Issue:** #1176

## Context

Wardian generates sibling `CLAUDE.md` bridges containing `@AGENTS.md` in its
managed instruction roots. A retained fresh Claude session stopped at external
import consent for the class and habitat `AGENTS.md` files. Additional-directory
access and instruction discovery did not remove that prompt. Claude treats
imports outside the working directory as requiring project approval; declining
disables those imports for that project. See [Claude's memory contract](https://code.claude.com/docs/en/memory#import-additional-files).

Wardian must load its own instruction wrappers through ordinary bootstrap while
retaining Claude's consent for user-authored external imports. Readiness and
approval detection are a separate concern under #1177.

## Decision and ownership

`AGENTS.md` remains Markdown-as-Truth. A managed sibling `CLAUDE.md` contains a
versioned HTML ownership comment with the SHA-256 of its following body, followed
by the canonical text verbatim. It is a derived provider view, not another
instruction source. The hash detects edits; it is not an authorization secret.

Only existing bridges at Wardian's common, selected class, and current agent
roots are eligible for refresh. The generated habitat also gets a projection.
Wardian replaces only its exact bare legacy `@AGENTS.md` stub or an unmodified
projection with the supported marker and matching body hash. Custom text,
modified markers or bodies, symbolic links, junctions, hardlinks, and other
unrecognized targets are preserved. Linked sources and managed include roots
are not followed. Missing or unreadable canonical content produces an error
instead of replacing an owned bridge with empty instructions.

Ordinary Claude habitat preparation refreshes the existing managed bridges.
Habitat generation writes its projection, and the runtime memory append refreshes
it again before launch. Publication replaces the complete sibling file; it does
not write through the canonical file. Projection I/O errors fail preparation.

Workspace files and user-selected include directories are outside the allowlist.
No global Claude configuration, trust setting, working directory, generic
provider filename, or approval response changes. Nested `@imports` remain literal
provider imports; Wardian neither reads their targets nor grants consent. A
sibling projection preserves the wrapper's relative import base. Existing
habitat aggregation rules remain unchanged.

## Freshness and tradeoff

Projections are bootstrap snapshots. Edit canonical `AGENTS.md` sources and start
a fresh session to load current instructions. The habitat already aggregates its
sources as a snapshot; no watcher or mid-session refresh guarantee is added.
Editing a generated `CLAUDE.md` makes it a preserved custom override. To return
an existing managed bridge to automatic refresh, deliberately restore its bare
`@AGENTS.md` stub; do not rewrite user workspace instruction files.

This avoids the provider wrapper import without granting project-wide external
import approval. Link-based bridges were rejected because of Windows privilege
requirements and the risk that bridge edits mutate canonical files. A copied
view requires refresh at bootstrap, which is the accepted freshness boundary.

## Validation boundary

Filesystem tests cover baseline stub failure, ordinary bootstrap migration,
post-memory refresh, ownership mutation, missing/unreadable sources, exact-root
selection, link preservation, and verbatim nested imports. These tests establish
file contents and ownership behavior, not Claude's actual loading behavior.

Serial real-provider acceptance must demonstrate known class and habitat markers
loading in a fresh session without the managed-wrapper consent menu, then an
untrusted external import remaining an operator approval with no automatic
acceptance. That acceptance is coordinated separately with #1177; no broad
consent or per-test trust configuration is part of this design.
