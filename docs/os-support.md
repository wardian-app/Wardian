# Platform Support & Compatibility

Wardian is built to be a high-performance terminal environment, leveraging native OS capabilities where possible.

---

## 🏆 Windows (Native Support)

Windows is the primary development and testing platform for Wardian.

- **Implementation**: Wardian uses `portable-pty` with native **ConPTY** support. This ensures a true, high-fidelity Windows terminal experience (supporting PowerShell 7, Git Bash, and CMD).
- **Nuance**: On Windows, Wardian handles UNC path resolution for Gemini logs and session state navigation, preventing common "path not found" errors when working across drive letters.
- **Recommended Tools**: We recommend using the latest PowerShell 7 for agent operations.

---

## ✅ macOS (Stable Support)

Wardian is fully supported on macOS, with a focus on Apple Silicon (M-series) performance.

- **Implementation**: Standard Unix PTY via `portable-pty`.
- **Status**: Stable. All core features (Library management, Workflows, Telemetry) are verified on macOS.
- **Visuals**: The UI includes macOS-specific titlebar and sidebar width optimizations for a more native look and feel.

---

## ✅ Linux (Stable Support)

Wardian is fully supported on Linux.

- **Implementation**: Standard Unix PTY via `portable-pty`.
- **Status**: Stable. Core features verified on Linux. Distribution-specific shell environments are handled via the runtime shell selection feature.
- **Packages**: `.AppImage` (portable, `chmod +x` and run) and `.deb` (install via `sudo apt install ./Wardian_*.deb`) are available from the Releases page. A Linux package-manager channel is tracked separately in [#324](https://github.com/wardian-app/Wardian/issues/324).

---

## Troubleshooting Terminal Issues

If a terminal appears frozen or fails to render:

1. **Check PTY Lifecycle**: If the agent's PTY dies, the status will turn **Gray (Off)**. Use the **Restart** action in the Command Matrix to re-initialize.
2. **Provider Logs**: Inspect the raw provider output using the **Dynamic Terminal Grid** (1x1 or focused view) to identify any CLI-level errors (e.g., authentication failure).
3. **Shell Compatibility**: Ensure the default shell on your system is supported by the agent provider you are using.
