# README Demo Capture

Use this workflow to refresh the animated demo shown near the top of the project README.

```bash
npm run docs:demo-gif
```

PowerShell:

```powershell
npm run docs:demo-gif
```

The capture script starts the native Wardian app, uses an isolated `WARDIAN_HOME`, seeds safe prompts, skills, classes, a workflow, and a tiny git repository, drives actual Wardian UI controls, and converts captured app frames to `public/demo.gif` with `ffmpeg`.

The filmed flow uses real providers inside the isolated demo workspace. By default it spawns a Claude agent, opens the single-agent context menu, creates a custom Antigravity clone, sends a terminal prompt to the clone, and runs a seeded Command-panel Quick Prompt against Claude. This keeps the demo focused on Wardian's GUI-first coordination surface instead of a single provider transcript.

The README hero demo should be filmed with real providers when the local machine has provider credentials and native WebDriver tooling available. It defaults to `WARDIAN_DEMO_SOURCE_PROVIDER=claude` and `WARDIAN_DEMO_CLONE_PROVIDER=antigravity`.

```bash
WARDIAN_DEMO_SOURCE_PROVIDER=claude WARDIAN_DEMO_CLONE_PROVIDER=antigravity npm run docs:demo-gif
```

PowerShell:

```powershell
$env:WARDIAN_DEMO_SOURCE_PROVIDER = "claude"
$env:WARDIAN_DEMO_CLONE_PROVIDER = "antigravity"
npm run docs:demo-gif
Remove-Item Env:\WARDIAN_DEMO_SOURCE_PROVIDER
Remove-Item Env:\WARDIAN_DEMO_CLONE_PROVIDER
```

The capture uses a fullscreen desktop viewport by default: `1920x1080`, exported to a `960px` wide GIF for README weight. Override with `WARDIAN_DEMO_VIEWPORT_WIDTH`, `WARDIAN_DEMO_VIEWPORT_HEIGHT`, and `WARDIAN_DEMO_WIDTH` only when the recording environment cannot provide the default size.

Runtime state and the demo workspace are created outside the Wardian checkout by default under a generic `wardian-demo-capture` directory. On Windows, the script prefers the public user directory when available; on macOS and Linux, it uses the OS temp directory. Override this with `WARDIAN_DEMO_RUNTIME_ROOT` when the default location is not appropriate.

`ffmpeg` must be available on `PATH`.

To keep generated frames for inspection:

```bash
WARDIAN_DEMO_KEEP_FRAMES=1 npm run docs:demo-gif
```

PowerShell:

```powershell
$env:WARDIAN_DEMO_KEEP_FRAMES = "1"
npm run docs:demo-gif
Remove-Item Env:\WARDIAN_DEMO_KEEP_FRAMES
```

Frames are written under `.tmp/readme-demo-real/frames`.
