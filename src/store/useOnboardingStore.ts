import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import type { OnboardingHintsState } from "../types/onboarding";

interface OnboardingState {
  dismissedHintIds: string[];
  hintsLoaded: boolean;
  loadOnboardingHints: () => Promise<void>;
  dismissOnboardingHint: (hintId: string) => Promise<void>;
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

export const useOnboardingStore = create<OnboardingState>((set, get) => ({
  dismissedHintIds: [],
  hintsLoaded: false,
  loadOnboardingHints: async () => {
    try {
      const state = await invoke<OnboardingHintsState>("load_onboarding_hints");
      set({
        dismissedHintIds: normalizeDismissedHintIds(state?.dismissed_hint_ids),
        hintsLoaded: true,
      });
    } catch (error) {
      console.error("Failed to load onboarding hints:", error);
      set({ dismissedHintIds: [], hintsLoaded: true });
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
        hintsLoaded: true,
      });
    } catch (error) {
      console.error("Failed to dismiss onboarding hint:", error);
      set({ dismissedHintIds: previousHintIds, hintsLoaded: true });
    }
  },
}));
