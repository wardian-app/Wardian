# README Demo Capture

Use this workflow to refresh the animated demo shown near the top of the project README.

```bash
npm run docs:demo-gif
```

PowerShell:

```powershell
npm run docs:demo-gif
```

The capture script starts the real Wardian frontend through Vite, installs a deterministic Tauri IPC fixture with Playwright, drives actual Wardian UI controls, and converts the captured app frames to `public/demo.gif` with `ffmpeg`. It intentionally uses fixture labels such as `<absolute-workspace-path>/demo-app` instead of local absolute paths, production sessions, credentials, or private repository names.

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

Frames are written under `.tmp/readme-demo/frames`.
