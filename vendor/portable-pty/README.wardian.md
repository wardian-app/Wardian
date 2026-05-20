# Wardian patch

This is `portable-pty` 0.9.0 with a small Windows-only ConPTY startup hint:
`STARTF_USESHOWWINDOW` + `SW_HIDE` is set when creating PTY child processes.

Wardian uses visible embedded PTYs for CLI providers. Some Windows CLI launches
can briefly surface a separate console window during provider restart/resume if
the process startup state is left to the default shell behavior. The hide hint
keeps the process attached to ConPTY while asking Windows not to show an
external console window.

Do not replace this vendored crate with the registry version unless the upstream
crate exposes equivalent behavior or Wardian has another Windows PTY window
policy.
