# Wardian vt100 fork

This directory vendors `vt100` 0.16.2 with a narrow OSC 8 preservation patch.
The patch stores bounded hyperlink targets on cells, carries them through the
existing grid and scrollback operations, and emits them from formatted screen
and row snapshots. The parser uses a bounded OSC buffer, caps each URI at 8
KiB, and reclaims hyperlink payload against a 2 MiB budget when the last cell
or active state reference is dropped.

Keep this fork small and rebase it if an upstream `vt100` release provides the
same cell and formatted output hooks.
