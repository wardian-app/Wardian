import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { spawnSync } from "node:child_process";

export class NativeArtifactResolutionError extends Error {
  constructor(message, { code = "NATIVE_ARTIFACT_RESOLUTION_FAILED", cause } = {}) {
    super(message, { cause });
    this.name = "NativeArtifactResolutionError";
    this.code = code;
  }
}

export function commandName(name, platform = process.platform) {
  return platform === "win32" ? `${name}.exe` : name;
}

function parseCargoMetadata(result, repoRoot) {
  if (result.error) {
    throw new NativeArtifactResolutionError(
      `Cargo metadata could not be started from ${repoRoot}: ${result.error.message}`,
      { code: "CARGO_METADATA_UNAVAILABLE", cause: result.error },
    );
  }

  if (result.status !== 0) {
    throw new NativeArtifactResolutionError(
      `Cargo metadata failed from ${repoRoot} (exit ${result.status ?? 1}).`,
      { code: "CARGO_METADATA_FAILED" },
    );
  }

  let metadata;
  try {
    metadata = JSON.parse(result.stdout);
  } catch (error) {
    throw new NativeArtifactResolutionError(
      `Cargo metadata returned invalid JSON from ${repoRoot}.`,
      { code: "CARGO_METADATA_INVALID", cause: error },
    );
  }

  if (typeof metadata.target_directory !== "string" || !metadata.target_directory.trim()) {
    throw new NativeArtifactResolutionError(
      `Cargo metadata did not provide target_directory for ${repoRoot}.`,
      { code: "CARGO_TARGET_DIRECTORY_MISSING" },
    );
  }

  return path.resolve(repoRoot, metadata.target_directory);
}

/**
 * Returns Cargo's effective target directory, including environment and
 * .cargo/config.toml overrides. Cargo normalizes relative target directories
 * in metadata, so callers must use this value instead of joining the raw
 * CARGO_TARGET_DIR string themselves.
 */
export function resolveCargoTargetDirectory({
  repoRoot,
  env = process.env,
  spawnSyncImpl = spawnSync,
} = {}) {
  if (!repoRoot) {
    throw new NativeArtifactResolutionError("repoRoot is required.", {
      code: "REPO_ROOT_MISSING",
    });
  }

  const root = path.resolve(repoRoot);
  const result = spawnSyncImpl("cargo", ["metadata", "--format-version=1", "--no-deps"], {
    cwd: root,
    encoding: "utf8",
    env,
  });
  return parseCargoMetadata(result, root);
}

/**
 * Resolves the CLI produced by the most recent Cargo build in this workspace.
 * The metadata target is authoritative; no stale repo-local fallback is used.
 */
export function resolveBuiltCliPath({
  repoRoot,
  env = process.env,
  platform = process.platform,
  spawnSyncImpl = spawnSync,
  existsSyncImpl = fs.existsSync,
} = {}) {
  const root = path.resolve(repoRoot || "");
  const targetDirectory = resolveCargoTargetDirectory({
    repoRoot: root,
    env,
    spawnSyncImpl,
  });
  const cliPath = path.join(targetDirectory, "debug", commandName("wardian-cli", platform));
  if (!existsSyncImpl(cliPath)) {
    throw new NativeArtifactResolutionError(
      `wardian-cli was not found at Cargo's effective target path: ${cliPath}.`,
      { code: "CLI_ARTIFACT_MISSING" },
    );
  }
  return cliPath;
}

/**
 * Resolves the CLI from Cargo's effective target when it already exists.
 *
 * Native harness setup runs before some callers build the CLI, so this keeps
 * the pre-build probe nullable without reintroducing a repo-local fallback.
 */
export function resolveExistingCliPath({
  repoRoot,
  env = process.env,
  platform = process.platform,
  spawnSyncImpl = spawnSync,
  existsSyncImpl = fs.existsSync,
} = {}) {
  const root = path.resolve(repoRoot || "");
  const targetDirectory = resolveCargoTargetDirectory({
    repoRoot: root,
    env,
    spawnSyncImpl,
  });
  const executableName = commandName("wardian-cli", platform);
  for (const profile of ["debug", "release"]) {
    const cliPath = path.join(targetDirectory, profile, executableName);
    if (existsSyncImpl(cliPath)) {
      return cliPath;
    }
  }
  return null;
}

/**
 * Resolves an explicitly supplied native app path relative to repoRoot, while
 * failing closed for missing paths and directories. Relative input is valid
 * because it is made deterministic before it reaches the native driver.
 */
export function resolveExplicitNativeApp({
  repoRoot,
  env = process.env,
  statSyncImpl = fs.statSync,
} = {}) {
  if (!Object.prototype.hasOwnProperty.call(env, "WARDIAN_NATIVE_APP")) {
    return null;
  }

  const rawValue = String(env.WARDIAN_NATIVE_APP ?? "").trim();
  if (!rawValue) {
    throw new NativeArtifactResolutionError("WARDIAN_NATIVE_APP was set but empty.", {
      code: "EXPLICIT_APP_EMPTY",
    });
  }

  const resolvedPath = path.resolve(repoRoot || process.cwd(), rawValue);
  let stat;
  try {
    stat = statSyncImpl(resolvedPath);
  } catch (error) {
    throw new NativeArtifactResolutionError(
      `WARDIAN_NATIVE_APP does not exist: ${resolvedPath}`,
      { code: "EXPLICIT_APP_MISSING", cause: error },
    );
  }

  if (!stat.isFile()) {
    throw new NativeArtifactResolutionError(
      `WARDIAN_NATIVE_APP must reference a regular app artifact: ${resolvedPath}`,
      { code: "EXPLICIT_APP_NOT_FILE" },
    );
  }

  return resolvedPath;
}

const APP_NAMES = Object.freeze({
  win32: ["Wardian.exe"],
  darwin: [
    path.join("bundle", "macos", "Wardian.app", "Contents", "MacOS", "Wardian"),
    path.join("Wardian.app", "Contents", "MacOS", "Wardian"),
  ],
  default: ["Wardian", "wardian"],
});

/**
 * Resolves an app from an explicit override or the effective Cargo target.
 * This helper is intentionally independent of the shared native session
 * lifecycle so port/home ownership can be integrated separately.
 */
export function resolveNativeAppArtifact({
  repoRoot,
  env = process.env,
  platform = process.platform,
  statSyncImpl = fs.statSync,
  spawnSyncImpl = spawnSync,
} = {}) {
  const explicit = resolveExplicitNativeApp({ repoRoot, env, statSyncImpl });
  if (explicit) {
    return { path: explicit, source: "explicit" };
  }

  const root = path.resolve(repoRoot || "");
  const targetDirectory = resolveCargoTargetDirectory({
    repoRoot: root,
    env,
    spawnSyncImpl,
  });
  const appNames = APP_NAMES[platform] || APP_NAMES.default;
  const candidates = [];
  for (const profile of ["debug", "release"]) {
    for (const appName of appNames) {
      candidates.push({
        path: path.join(targetDirectory, profile, appName),
        profile,
      });
    }
  }

  for (const candidate of candidates) {
    try {
      if (statSyncImpl(candidate.path).isFile()) {
        return { path: candidate.path, profile: candidate.profile, source: "cargo-target" };
      }
    } catch {
      // Continue through all supported artifact names before failing clearly.
    }
  }

  throw new NativeArtifactResolutionError(
    `Wardian native app was not found under Cargo's effective target directory: ${targetDirectory}.`,
    { code: "APP_ARTIFACT_MISSING" },
  );
}

export function resolvePackageEntry({
  repoRoot,
  packageName,
  requireResolve = createRequire(path.join(path.resolve(repoRoot || ""), "package.json")).resolve,
} = {}) {
  if (!packageName) {
    throw new NativeArtifactResolutionError("packageName is required.", {
      code: "PACKAGE_NAME_MISSING",
    });
  }

  try {
    return requireResolve(packageName);
  } catch (error) {
    throw new NativeArtifactResolutionError(
      `Required Node package is unavailable: ${packageName}. Run the project dependency setup in this worktree.`,
      { code: "NODE_PACKAGE_MISSING", cause: error },
    );
  }
}

export function nativeNodeDependencyDiagnostics({
  repoRoot,
  requireResolve,
} = {}) {
  const dependencies = ["@tauri-apps/cli/tauri.js", "selenium-webdriver"];
  const missing = [];
  const resolved = {};
  for (const packageName of dependencies) {
    try {
      resolved[packageName] = resolvePackageEntry({ repoRoot, packageName, requireResolve });
    } catch (error) {
      missing.push({ packageName, code: error.code, message: error.message });
    }
  }
  return { ok: missing.length === 0, missing, resolved };
}

export function assertNativeNodeDependencies(options = {}) {
  const diagnostics = nativeNodeDependencyDiagnostics(options);
  if (!diagnostics.ok) {
    const detail = diagnostics.missing.map(({ message }) => message).join(" ");
    throw new NativeArtifactResolutionError(detail, { code: "NODE_DEPENDENCIES_MISSING" });
  }
  return diagnostics.resolved;
}
