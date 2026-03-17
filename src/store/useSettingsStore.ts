import { create } from 'zustand';
import { persist } from 'zustand/middleware';

interface SettingsState {
  theme: 'dark' | 'light' | 'system';
  autoPatchGemini: boolean;
  setTheme: (theme: 'dark' | 'light' | 'system') => void;
  setAutoPatchGemini: (enabled: boolean) => void;
}

export const useSettingsStore = create<SettingsState>()(
  persist(
    (set) => ({
      theme: 'system',
      autoPatchGemini: false,
      setTheme: (theme) => set({ theme }),
      setAutoPatchGemini: (autoPatchGemini) => set({ autoPatchGemini }),
    }),
    {
      name: 'wardian-settings', // name of the item in the storage (must be unique)
    }
  )
);
