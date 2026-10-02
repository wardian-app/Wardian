# Wardian patch

This is `portable-pty` 0.9.0 with a small Windows-only ConPTY startup patch:
`STARTF_USESHOWWINDOW` + `SW_HIDE` is set when creating PTY child processes.

Wardian uses visible embedded PTYs for CLI providers. Some Windows CLI launches
can briefly surface a separate console window during provider restart/resume if
the process startup state is left to the default shell behavior. The hide hint
keeps the process attached to ConPTY while asking Windows not to show an
external console window.

Do not replace this vendored crate with the registry version unless the upstream
crate exposes equivalent behavior or Wardian has another Windows PTY window
policy.

Windows `CommandBuilder::set_windows_job` duplicates a supplied job handle.
When present, ConPTY creates the child suspended, assigns the job before any
provider code executes, then resumes its main thread. Assignment or resume
failure terminates the suspended child and returns a launch error. Builders
without a job preserve the existing creation flags. The retained job belongs
to Wardian's runtime, allowing verified tree stop during Claude rotation.
