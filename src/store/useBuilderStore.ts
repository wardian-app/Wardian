import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { Blueprint, Diagnostic } from '../features/workflows/builder/blueprintTypes';

interface BuilderState {
  blueprint: Blueprint | null;
  /** Last successfully loaded/saved value used by Discard close guards. */
  baseline: Blueprint | null;
  baselineDiagnostics: Diagnostic[];
  path: string | null;
  diagnostics: Diagnostic[];
  dirty: boolean;
  editRevision: number;
  load: (path: string) => Promise<void>;
  validate: () => Promise<void>;
  save: () => Promise<boolean>;
  initialize: (bp: Blueprint) => void;
  setBlueprint: (bp: Blueprint) => void;
  discard: () => boolean;
  hasErrors: () => boolean;
  reset: () => void;
}

export const useBuilderStore = create<BuilderState>((set, get) => ({
  blueprint: null,
  baseline: null,
  baselineDiagnostics: [],
  path: null,
  diagnostics: [],
  dirty: false,
  editRevision: 0,
  async load(path) {
    const res = await invoke<{ blueprint: Blueprint; diagnostics: Diagnostic[] }>('workflow_parse', { path });
    set({
      blueprint: res.blueprint,
      baseline: res.blueprint,
      baselineDiagnostics: res.diagnostics,
      path,
      diagnostics: res.diagnostics,
      dirty: false,
      editRevision: 0,
    });
  },
  async validate() {
    const bp = get().blueprint;
    if (!bp) return;
    const res = await invoke<{ ok: boolean; diagnostics: Diagnostic[] }>('workflow_validate', { blueprint: bp });
    set({ diagnostics: res.diagnostics });
  },
  async save() {
    const { blueprint, path, editRevision } = get();
    if (!blueprint || !path) return false;
    const res = await invoke<{ written: boolean; diagnostics: Diagnostic[] }>('workflow_write', { path, blueprint });
    if (get().path !== path) return false;
    const editChangedDuringSave = get().editRevision !== editRevision
      || get().blueprint !== blueprint;
    set({
      diagnostics: res.diagnostics,
      dirty: !res.written || editChangedDuringSave,
      ...(res.written ? { baseline: blueprint, baselineDiagnostics: res.diagnostics } : {}),
    });
    return res.written && !editChangedDuringSave;
  },
  initialize(bp) {
    if (get().blueprint) return;
    set({ blueprint: bp, baseline: bp, baselineDiagnostics: [], dirty: false, editRevision: 0 });
  },
  setBlueprint(bp) {
    set((state) => ({ blueprint: bp, dirty: true, editRevision: state.editRevision + 1 }));
  },
  discard() {
    const { baseline, baselineDiagnostics } = get();
    set((state) => ({
      blueprint: baseline,
      diagnostics: baselineDiagnostics,
      dirty: false,
      editRevision: state.editRevision + 1,
    }));
    return true;
  },
  hasErrors() { return get().diagnostics.some((d) => d.severity === 'error'); },
  reset() {
    set({
      blueprint: null,
      baseline: null,
      baselineDiagnostics: [],
      path: null,
      diagnostics: [],
      dirty: false,
      editRevision: 0,
    });
  },
}));
