import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import tauriConfig from "../../src-tauri/tauri.conf.json?raw";
import macosConfig from "../../src-tauri/tauri.macos.conf.json?raw";
import capabilities from "../../src-tauri/capabilities/default.json?raw";
import settingsCommands from "../../src-tauri/src/commands/settings.rs?raw";
import appRuntime from "../../src-tauri/src/lib.rs?raw";
import macosRuntime from "../../src-tauri/src/macos_window.rs?raw";

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

  it("uses native macOS traffic lights positioned in Wardian's top overlay row", () => {
    const macWindow = JSON.parse(macosConfig).app.windows[0];

    expect(macWindow).toMatchObject({
      decorations: true,
      titleBarStyle: "Overlay",
      hiddenTitle: true,
      trafficLightPosition: { x: 14, y: 11 },
    });
  });

  it("uses a dedicated full-canvas app icon for macOS", () => {
    const bundle = JSON.parse(macosConfig).bundle;

    expect(bundle.icon).toEqual(["icons/macos/icon.icns"]);
    expect(existsSync(resolve("src-tauri/icons/macos/icon.icns"))).toBe(true);
    expect(readFileSync(resolve("src-tauri/icons/macos/icon.icns")).equals(
      readFileSync(resolve("src-tauri/icons/white/icon.icns")),
    )).toBe(false);
    expect(readFileSync(resolve("src-tauri/icons/macos/icon.svg"), "utf8")).toContain(
      '<rect width="1024" height="1024" fill="url(#background)"/>',
    );
  });

  it("keeps the green traffic light in Wardian's window by using zoom instead of a full-screen Space", () => {
    expect(appRuntime).toContain("crate::macos_window::configure_main_window(app)?;");
    expect(macosRuntime).toContain("NSWindowCollectionBehavior::FullScreenNone");
    expect(macosRuntime).toContain("NSWindowCollectionBehavior::FullScreenPrimary");
  });

  it("restarts from the native runtime instead of the browser-side process plugin", () => {
    expect(settingsCommands).toContain("pub fn restart_app(app: tauri::AppHandle)");
    expect(settingsCommands).toContain("app.restart();");
    expect(appRuntime).toContain("commands::settings::restart_app,");
    expect(appRuntime).not.toContain("tauri_plugin_process::init()");
    expect(capabilities).not.toContain("process:allow-restart");
  });
});
