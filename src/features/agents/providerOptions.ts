import type { DefaultProviderSetting } from "../../types/settings";
import type { ProviderReadiness, UserFacingProviderName } from "../../types";

export const PROVIDER_ORDER: UserFacingProviderName[] = ["claude", "codex", "gemini", "antigravity", "opencode"];

export interface ProviderOption {
  value: UserFacingProviderName;
  label: string;
  available: boolean;
  reason: string | null;
}

export function providerDisplayName(provider: UserFacingProviderName): string {
  switch (provider) {
    case "claude":
      return "Claude";
    case "codex":
      return "Codex";
    case "gemini":
      return "Gemini";
    case "antigravity":
      return "antigravity";
    case "opencode":
      return "OpenCode";
  }
}

export function isUserFacingProviderName(provider: string | null | undefined): provider is UserFacingProviderName {
  return PROVIDER_ORDER.includes(provider as UserFacingProviderName);
}

export function buildProviderOptions(readiness: ProviderReadiness[]): ProviderOption[] {
  const byProvider = new Map(readiness.map((entry) => [entry.provider, entry]));
  return PROVIDER_ORDER.map((provider) => {
    const entry = byProvider.get(provider);
    const available = entry?.available ?? false;
    const label = entry?.display_name ?? providerDisplayName(provider);
    return {
      value: provider,
      label: available ? label : `${label} - not installed`,
      available,
      reason: entry?.reason ?? null,
    };
  });
}

export function buildUngatedProviderOptions(): ProviderOption[] {
  return PROVIDER_ORDER.map((provider) => ({
    value: provider,
    label: providerDisplayName(provider),
    available: true,
    reason: null,
  }));
}

export function resolveEffectiveProvider(
  readiness: ProviderReadiness[],
  defaultProvider: DefaultProviderSetting,
): { provider: UserFacingProviderName | null; note: string | null } {
  const options = buildProviderOptions(readiness);
  const firstAvailable = options.find((option) => option.available)?.value ?? null;

  if (!firstAvailable) {
    return { provider: null, note: "No supported provider CLI was found." };
  }

  if (defaultProvider !== "auto") {
    const explicit = options.find((option) => option.value === defaultProvider);
    if (explicit?.available) {
      return { provider: explicit.value, note: null };
    }

    return {
      provider: firstAvailable,
      note: `Default provider ${providerDisplayName(defaultProvider)} is not installed. Using ${providerDisplayName(firstAvailable)}.`,
    };
  }

  return { provider: firstAvailable, note: null };
}
