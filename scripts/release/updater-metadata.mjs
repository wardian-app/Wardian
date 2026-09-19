const updaterBundles = [
  {
    assetName: () => "Wardian_aarch64.app.tar.gz",
    platforms: ["darwin-aarch64", "darwin-aarch64-app"],
  },
  {
    assetName: () => "Wardian_x64.app.tar.gz",
    platforms: ["darwin-x86_64", "darwin-x86_64-app"],
  },
  {
    assetName: (version) => `Wardian_${version}_amd64.AppImage`,
    platforms: ["linux-x86_64", "linux-x86_64-appimage"],
  },
  {
    assetName: (version) => `Wardian_${version}_amd64.deb`,
    platforms: ["linux-x86_64-deb"],
  },
  {
    assetName: (version) => `Wardian_${version}_x64-setup.exe`,
    platforms: ["windows-x86_64", "windows-x86_64-nsis"],
  },
];

function releaseVersion(tag) {
  const version = tag.replace(/^v/, "");
  if (!version || version === tag) {
    throw new Error(`Updater release tag must start with v: ${tag || "<missing>"}`);
  }
  return version;
}

function bundleDescriptors(tag) {
  const version = releaseVersion(tag);
  return updaterBundles.map((bundle) => {
    const assetName = bundle.assetName(version);
    return {
      ...bundle,
      assetName,
      signatureName: `${assetName}.sig`,
    };
  });
}

/** Returns the signature assets required to assemble stable updater metadata. */
export function requiredUpdaterSignatureNames(tag) {
  return bundleDescriptors(tag).map((bundle) => bundle.signatureName);
}

/**
 * Builds complete Tauri updater metadata from one release asset inventory.
 * Matrix jobs upload bundles and signatures; one post-build job calls this
 * function so no two jobs can replace latest.json concurrently.
 */
export function buildUpdaterMetadata({
  tag,
  repository,
  assetNames,
  signatures,
  pubDate = new Date().toISOString(),
}) {
  const version = releaseVersion(tag);
  if (!/^[^/]+\/[^/]+$/.test(repository)) {
    throw new Error(`Invalid GitHub repository: ${repository || "<missing>"}`);
  }

  const availableAssets = new Set(assetNames);
  if (availableAssets.size !== assetNames.length) {
    throw new Error("Release asset names must be unique");
  }

  const platforms = {};
  for (const bundle of bundleDescriptors(tag)) {
    if (!availableAssets.has(bundle.assetName)) {
      throw new Error(`Missing updater asset: ${bundle.assetName}`);
    }
    if (!availableAssets.has(bundle.signatureName)) {
      throw new Error(`Missing updater signature asset: ${bundle.signatureName}`);
    }

    const signature = signatures[bundle.signatureName]?.trim();
    if (!signature) {
      throw new Error(`Updater signature is empty: ${bundle.signatureName}`);
    }

    const url = `https://github.com/${repository}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(bundle.assetName)}`;
    for (const platform of bundle.platforms) {
      platforms[platform] = { signature, url };
    }
  }

  return {
    version,
    notes: "",
    pub_date: pubDate,
    platforms,
  };
}
