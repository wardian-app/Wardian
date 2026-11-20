# Consistent Chat Transcript Width

## Decision

Updated 2026-11-07: use one responsive outer transcript lane on desktop and
remote surfaces. It is fluid up to `72rem`, then remains centered. The composer
shares that lane and its gutters. Prose has an inner `100ch` readable limit;
technical content can use the full lane.

All rendered transcript rows—messages, tool activity, terminal fallbacks,
work logs, and change summaries—must be children of that column. Individual
message bubbles retain role-specific sizing. Code, tables, diffs and work logs
use the available technical width, with wrapping or local overflow rather than
horizontal overflow of the whole document. Narrow panes retain their gutters
and touch-sized message actions.

## Rationale

The original shared `76ch` column kept rows aligned but constrained code and
tables unnecessarily. A common outer lane preserves alignment while separate
prose and technical limits make wide panes useful without stretching paragraphs
across the screen. Both limits remain fluid on phones and narrow panes.

Width does not change paging authority. The PWA's optional recent target is
eight headers, with an omitted target retaining 80 and atomic groups bounded
by the hard 80-header cap. Older, detail and revalidation keep their existing
budgets; see [recent header windows](../developer/remote-gateway.md#recent-header-windows).

## Verification

- At a wide viewport, rows and composer share the centered lane up to `72rem`;
  prose stays within `100ch` while technical content can use the wider lane.
- Below those limits, content fits the available pane without document overflow.
- Code and tables keep any necessary overflow local to their own container.
- Phone gutters remain visible and message-action targets are at least 44 CSS px.
- Desktop and remote chat use the same transcript-width class.
