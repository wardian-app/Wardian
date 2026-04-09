import { invoke } from '@tauri-apps/api/core';
import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type { ShellOption, ShellSettings } from '../types/settings';

interface SettingsState {
  theme: 'dark' | 'light' | 'system';
  autoPatchGemini: boolean;
  shell_id: string;
  custom_executable: string;
  custom_args: string;
  available_shells: ShellOption[];
  shell_settings_loaded: boolean;
  shells_loaded: boolean;
  setTheme: (theme: 'dark' | 'light' | 'system') => void;
  setAutoPatchGemini: (enabled: boolean) => void;
  setShellId: (shellId: string) => void;
  setCustomExecutable: (value: string) => void;
  setCustomArgs: (value: string) => void;
  loadShellSettings: () => Promise<void>;
  loadAvailableShells: () => Promise<void>;
  saveShellSettings: () => Promise<void>;
}

const DEFAULT_SHELL_SETTINGS: ShellSettings = {
  shell_id: 'auto',
  custom_executable: null,
  custom_args: null,
};

export const useSettingsStore = create<SettingsState>()(
  persist(
    (set, get) => ({
      theme: 'system',
      autoPatchGemini: false,
      shell_id: DEFAULT_SHELL_SETTINGS.shell_id,
      custom_executable: DEFAULT_SHELL_SETTINGS.custom_executable ?? '',
      custom_args: DEFAULT_SHELL_SETTINGS.custom_args ?? '',
      available_shells: [],
      shell_settings_loaded: false,
      shells_loaded: false,
      setTheme: async (theme) => {
        set({ theme });
      },
      setAutoPatchGemini: (autoPatchGemini) => set({ autoPatchGemini }),
      setShellId: (shell_id) => set({ shell_id }),
      setCustomExecutable: (custom_executable) => set({ custom_executable }),
      setCustomArgs: (custom_args) => set({ custom_args }),
      loadShellSettings: async () => {
        try {
          const settings = await invoke<ShellSettings>('load_shell_settings');
          set({
            shell_id: settings.shell_id,
            custom_executable: settings.custom_executable ?? '',
            custom_args: settings.custom_args ?? '',
            shell_settings_loaded: true,
          });
        } catch (error) {
          console.error('Failed to load shell settings:', error);
          set({
            shell_id: DEFAULT_SHELL_SETTINGS.shell_id,
            custom_executable: '',
            custom_args: '',
            shell_settings_loaded: true,
          });
        }
      },
      loadAvailableShells: async () => {
        try {
          const available_shells = await invoke<ShellOption[]>('list_available_shells');
          set({ available_shells, shells_loaded: true });
        } catch (error) {
          console.error('Failed to load available shells:', error);
          set({ available_shells: [], shells_loaded: true });
        }
      },
      saveShellSettings: async () => {
        const settings: ShellSettings = {
          shell_id: get().shell_id,
          custom_executable: get().custom_executable.trim() || null,
          custom_args: get().custom_args.trim() || null,
        };
        const saved = await invoke<ShellSettings>('save_shell_settings', { settings });
        set({
          shell_id: saved.shell_id,
          custom_executable: saved.custom_executable ?? '',
          custom_args: saved.custom_args ?? '',
          shell_settings_loaded: true,
        });
      },
    }),
    {
      name: 'wardian-settings',
      partialize: (state) => ({
        theme: state.theme,
        autoPatchGemini: state.autoPatchGemini,
      }),
    }
  )
);
