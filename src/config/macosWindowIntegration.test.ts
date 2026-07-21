import { describe, expect, it } from "vitest";
import tauriConfig from "../../src-tauri/tauri.conf.json?raw";
import macosConfig from "../../src-tauri/tauri.macos.conf.json?raw";
import capabilities from "../../src-tauri/capabilities/default.json?raw";
import settingsCommands from "../../src-tauri/src/commands/settings.rs?raw";
import appRuntime from "../../src-tauri/src/lib.rs?raw";

describe("macOS window integration", () => {
  it("keeps the transparent icon family for non-macOS builds", () => {
    const bundle = JSON.parse(tauriConfig).bundle;

    expect(bundle.icon).toEqual([
      "icons/transparent/32x32.png",
      "icons/transparent/128x128.png",
      "icons/transparent/128x128@2x.png",
      "icons/transparent/icon.icns",
      "icons/transparent/icon.ico",
    ]);
  });

  it("uses native macOS traffic lights positioned inside Wardian's titlebar", () => {
    const macWindow = JSON.parse(macosConfig).app.windows[0];

    expect(macWindow).toMatchObject({
      decorations: true,
      titleBarStyle: "Overlay",
      hiddenTitle: true,
      trafficLightPosition: { x: 14, y: 11 },
    });
  });

  it("uses the opaque white app icon family for macOS instead of a system-composited transparent icon", () => {
    const bundle = JSON.parse(macosConfig).bundle;

    expect(bundle.icon).toEqual([
      "icons/white/32x32.png",
      "icons/white/128x128.png",
      "icons/white/128x128@2x.png",
      "icons/white/icon.icns",
      "icons/white/icon.ico",
    ]);
  });

  it("restarts from the native runtime instead of the browser-side process plugin", () => {
    expect(settingsCommands).toContain("pub fn restart_app(app: tauri::AppHandle)");
    expect(settingsCommands).toContain("app.restart();");
    expect(appRuntime).toContain("commands::settings::restart_app,");
    expect(appRuntime).not.toContain("tauri_plugin_process::init()");
    expect(capabilities).not.toContain("process:allow-restart");
  });
});
