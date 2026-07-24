import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import type { OnboardingHintsState } from "../types/onboarding";

interface OnboardingState {
  dismissedHintIds: string[];
  contextualTipsEnabled: boolean;
  hintsLoaded: boolean;
  loadOnboardingHints: () => Promise<void>;
  dismissOnboardingHint: (hintId: string) => Promise<void>;
  setContextualTipsEnabled: (enabled: boolean) => Promise<void>;
  resetOnboardingHints: () => Promise<void>;
}

function normalizeDismissedHintIds(value: unknown): string[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return Array.from(
    new Set(
      value
        .filter((hintId): hintId is string => typeof hintId === "string")
        .map((hintId) => hintId.trim())
        .filter(Boolean),
    ),
  ).sort();
}

function normalizeContextualTipsEnabled(value: unknown): boolean {
  return value !== false;
}

export const useOnboardingStore = create<OnboardingState>((set, get) => ({
  dismissedHintIds: [],
  contextualTipsEnabled: true,
  hintsLoaded: false,
  loadOnboardingHints: async () => {
    try {
      const state = await invoke<OnboardingHintsState>("load_onboarding_hints");
      set({
        dismissedHintIds: normalizeDismissedHintIds(state?.dismissed_hint_ids),
        contextualTipsEnabled: normalizeContextualTipsEnabled(state?.contextual_tips_enabled),
        hintsLoaded: true,
      });
    } catch (error) {
      console.error("Failed to load onboarding hints:", error);
      set({ dismissedHintIds: [], contextualTipsEnabled: true, hintsLoaded: true });
    }
  },
  dismissOnboardingHint: async (hintId) => {
    const previousHintIds = get().dismissedHintIds;
    const nextHintIds = normalizeDismissedHintIds([...previousHintIds, hintId]);
    set({ dismissedHintIds: nextHintIds, hintsLoaded: true });

    try {
      const state = await invoke<OnboardingHintsState>("dismiss_onboarding_hint", { hintId });
      set({
        dismissedHintIds: normalizeDismissedHintIds(state?.dismissed_hint_ids),
        contextualTipsEnabled: normalizeContextualTipsEnabled(state?.contextual_tips_enabled),
        hintsLoaded: true,
      });
    } catch (error) {
      console.error("Failed to dismiss onboarding hint:", error);
      set({ dismissedHintIds: previousHintIds, hintsLoaded: true });
    }
  },
  setContextualTipsEnabled: async (enabled) => {
    const previousEnabled = get().contextualTipsEnabled;
    set({ contextualTipsEnabled: enabled, hintsLoaded: true });

    try {
      const state = await invoke<OnboardingHintsState>("set_contextual_tips_enabled", { enabled });
      set({
        dismissedHintIds: normalizeDismissedHintIds(state?.dismissed_hint_ids),
        contextualTipsEnabled: normalizeContextualTipsEnabled(state?.contextual_tips_enabled),
        hintsLoaded: true,
      });
    } catch (error) {
      console.error("Failed to update contextual tips preference:", error);
      set({ contextualTipsEnabled: previousEnabled, hintsLoaded: true });
    }
  },
  resetOnboardingHints: async () => {
    const previousHintIds = get().dismissedHintIds;
    set({ dismissedHintIds: [], hintsLoaded: true });

    try {
      const state = await invoke<OnboardingHintsState>("reset_onboarding_hints");
      set({
        dismissedHintIds: normalizeDismissedHintIds(state?.dismissed_hint_ids),
        contextualTipsEnabled: normalizeContextualTipsEnabled(state?.contextual_tips_enabled),
        hintsLoaded: true,
      });
    } catch (error) {
      console.error("Failed to reset onboarding hints:", error);
      set({ dismissedHintIds: previousHintIds, hintsLoaded: true });
    }
  },
}));
