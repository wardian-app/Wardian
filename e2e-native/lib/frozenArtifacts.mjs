import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { resolvePackagedNativePair, validatePairedNativeCli } from "./native-artifact-resolution.mjs";

/** Directory inside a run's home holding that run's private binaries. */
export const FROZEN_BIN_DIR = ".frozen-bin";
const RUNTIME_PAYLOAD_DIRECTORIES = ["conpty", "resources"];

function sha256(filePath) {
  return crypto.createHash("sha256").update(fs.readFileSync(filePath)).digest("hex");
}

/**
 * Binaries a Windows Tauri build needs beside the executable.
 *
 * Copying the executable alone produces something that cannot start, so the
 * adjacent link libraries travel with it. Tauri's allowlisted runtime payload
 * directories are copied below without replacing files already frozen for the
 * run, because a later CLI freeze may share this destination after launch.
 */
function sidecarsFor(sourcePath) {
  const dir = path.dirname(sourcePath);
  let entries;
  try {
    entries = fs.readdirSync(dir, { withFileTypes: true });
  } catch {
    return [];
  }
  return entries
    .filter((entry) => entry.isFile() && /\.(dll|so|dylib)$/i.test(entry.name))
    .map((entry) => path.join(dir, entry.name));
}

/** Runtime directories resolved relative to a Tauri executable. */
function runtimePayloadsFor(sourcePath) {
  const dir = path.dirname(sourcePath);
  return RUNTIME_PAYLOAD_DIRECTORIES
    .map((name) => path.join(dir, name))
    .filter((payloadPath) => fs.existsSync(payloadPath) && fs.statSync(payloadPath).isDirectory());
}

function copyMissingTree(sourcePath, destPath) {
  if (!fs.statSync(sourcePath).isDirectory()) {
    if (!fs.existsSync(destPath)) {
      fs.copyFileSync(sourcePath, destPath);
    }
    return;
  }
  if (fs.existsSync(destPath) && !fs.statSync(destPath).isDirectory()) {
    return;
  }
  // Node's Windows cpSync can misencode Unicode destination paths.
  fs.mkdirSync(destPath, { recursive: true });
  for (const entry of fs.readdirSync(sourcePath, { withFileTypes: true })) {
    copyMissingTree(path.join(sourcePath, entry.name), path.join(destPath, entry.name));
  }
}

/**
 * Copy one binary and its sidecars into a run-private directory.
 *
 * Recording a hash of the original only attributes what a run started with; it
 * does not stop another worktree rebuilding that path midway and changing the
 * bytes underneath a live session. The normal build target is shared, so a run
 * takes its own copy and executes that instead. The recorded identity ties the
 * copy back to the source it came from.
 * `includeRuntime=false` copies only a paired CLI; its app owns the runtime.
 */
export function freezeArtifact(sourcePath, destDir, { includeRuntime = true } = {}) {
  if (!sourcePath || !fs.existsSync(sourcePath)) {
    return null;
  }
  fs.mkdirSync(destDir, { recursive: true });

  if (includeRuntime) {
    for (const sidecar of sidecarsFor(sourcePath)) {
      const target = path.join(destDir, path.basename(sidecar));
      if (!fs.existsSync(target)) {
        fs.copyFileSync(sidecar, target);
      }
    }
    for (const payload of runtimePayloadsFor(sourcePath)) {
      const target = path.join(destDir, path.basename(payload));
      copyMissingTree(payload, target);
    }
  }

  const frozenPath = path.join(destDir, path.basename(sourcePath));
  fs.copyFileSync(sourcePath, frozenPath);
  const stats = fs.statSync(frozenPath);
  return {
    path: frozenPath,
    source: sourcePath,
    sha256: sha256(frozenPath),
    bytes: stats.size,
    frozenAt: new Date().toISOString(),
  };
}

/**
 * Freeze every binary a run executes, into that run's own home.
 *
 * The home is per-run and is removed with the run, so the copies are cleaned up
 * without any extra bookkeeping.
 * App-backed runs use the app's prelaunch packaged CLI. An explicit pair keeps
 * its caller-selected CLI and must match that package. Both copied pairs are
 * checked again; a standalone CLI source never contributes to an app runtime.
 */
export function freezeRunArtifacts({ home, appPath, cliPath, pairedCli = false, platform = process.platform }) {
  const pair = pairedCli ? validatePairedNativeCli({ appPath, cliPath, platform })
    : appPath ? resolvePackagedNativePair({ appPath, platform }) : null;
  const destDir = path.join(home, FROZEN_BIN_DIR);
  const app = freezeArtifact(pair?.appPath ?? appPath, destDir);
  if (pair) {
    // Preserve the app loader's POSIX resource layout in this owned home.
    // Cargo-output resource roots also contain build outputs: copy only the
    // app's resource folders, never the whole shared compiler directory.
    const resourceDest = platform === "win32" ? destDir
      : platform === "darwin" ? path.join(home, "Resources") : path.join(home, "lib", "Wardian");
    if (pair.resourceDir === path.dirname(pair.appPath)) {
      const directories = ["bin", "resources"];
      if (platform === "win32") {
        // Tauri declares these alongside the executable, including the nested
        // parent-relative scripts layout. The compiler directory stays bounded.
        directories.push("agent_prompts", path.join("_up_", "scripts"));
      }
      for (const name of directories) {
        const source = path.join(pair.resourceDir, name);
        if (fs.existsSync(source) && fs.statSync(source).isDirectory()) {
          copyMissingTree(source, path.join(resourceDest, name));
        }
      }
    } else {
      copyMissingTree(pair.resourceDir, resourceDest);
    }
  }
  // The paired app supplies the runtime. Importing the CLI's adjacent files
  // would silently mix builds even when the CLI executable itself matches.
  const cliSource = pair?.cliPath ?? cliPath;
  const cli = cliSource ? freezeArtifact(cliSource, destDir, { includeRuntime: !pair }) : null;
  if (pair) validatePairedNativeCli({ appPath: app.path, cliPath: cli.path, platform });
  return { dir: destDir, app, cli };
}
