import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { Blueprint, Diagnostic } from '../features/workflows/builder/blueprintTypes';

interface BuilderState {
  blueprint: Blueprint | null;
  path: string | null;
  diagnostics: Diagnostic[];
  dirty: boolean;
  load: (path: string) => Promise<void>;
  validate: () => Promise<void>;
  save: () => Promise<boolean>;
  setBlueprint: (bp: Blueprint) => void;
  hasErrors: () => boolean;
  reset: () => void;
}

export const useBuilderStore = create<BuilderState>((set, get) => ({
  blueprint: null,
  path: null,
  diagnostics: [],
  dirty: false,
  async load(path) {
    const res = await invoke<{ blueprint: Blueprint; diagnostics: Diagnostic[] }>('workflow_parse', { path });
    set({ blueprint: res.blueprint, path, diagnostics: res.diagnostics, dirty: false });
  },
  async validate() {
    const bp = get().blueprint;
    if (!bp) return;
    const res = await invoke<{ ok: boolean; diagnostics: Diagnostic[] }>('workflow_validate', { blueprint: bp });
    set({ diagnostics: res.diagnostics });
  },
  async save() {
    const { blueprint, path } = get();
    if (!blueprint || !path) return false;
    const res = await invoke<{ written: boolean; diagnostics: Diagnostic[] }>('workflow_write', { path, blueprint });
    set({ diagnostics: res.diagnostics, dirty: !res.written });
    return res.written;
  },
  setBlueprint(bp) { set({ blueprint: bp, dirty: true }); },
  hasErrors() { return get().diagnostics.some((d) => d.severity === 'error'); },
  reset() { set({ blueprint: null, path: null, diagnostics: [], dirty: false }); },
}));
