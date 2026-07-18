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
  /** Monotonic identity/content revision for close preparation and deferred effects. */
  resourceRevision: number;
  /** Monotonic workflow identity token used to reject stale async save responses. */
  resourceIdentityRevision: number;
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
  resourceRevision: 0,
  resourceIdentityRevision: 0,
  async load(path) {
    const res = await invoke<{ blueprint: Blueprint; diagnostics: Diagnostic[] }>('workflow_parse', { path });
    set((state) => ({
      blueprint: res.blueprint,
      baseline: res.blueprint,
      baselineDiagnostics: res.diagnostics,
      path,
      diagnostics: res.diagnostics,
      dirty: false,
      editRevision: 0,
      resourceRevision: state.resourceRevision + 1,
      resourceIdentityRevision: state.resourceIdentityRevision + 1,
    }));
  },
  async validate() {
    const bp = get().blueprint;
    if (!bp) return;
    const res = await invoke<{ ok: boolean; diagnostics: Diagnostic[] }>('workflow_validate', { blueprint: bp });
    set({ diagnostics: res.diagnostics });
  },
  async save() {
    const { blueprint, path, editRevision, resourceIdentityRevision } = get();
    if (!blueprint || !path) return false;
    const res = await invoke<{ written: boolean; diagnostics: Diagnostic[] }>('workflow_write', { path, blueprint });
    if (
      get().path !== path
      || get().resourceIdentityRevision !== resourceIdentityRevision
      || get().blueprint?.id !== blueprint.id
    ) return false;
    const editChangedDuringSave = get().editRevision !== editRevision
      || get().blueprint !== blueprint;
    set((state) => ({
      diagnostics: res.diagnostics,
      dirty: !res.written || editChangedDuringSave,
      ...(res.written ? { baseline: blueprint, baselineDiagnostics: res.diagnostics } : {}),
      resourceRevision: state.resourceRevision + 1,
    }));
    return res.written && !editChangedDuringSave;
  },
  initialize(bp) {
    set((state) => state.blueprint ? {} : {
      blueprint: bp,
      baseline: bp,
      baselineDiagnostics: [],
      dirty: false,
      editRevision: 0,
      resourceRevision: state.resourceRevision + 1,
      resourceIdentityRevision: state.resourceIdentityRevision + 1,
    });
  },
  setBlueprint(bp) {
    set((state) => ({
      blueprint: bp,
      dirty: true,
      editRevision: state.editRevision + 1,
      resourceRevision: state.resourceRevision + 1,
      ...(state.blueprint?.id === bp.id
        ? {}
        : { resourceIdentityRevision: state.resourceIdentityRevision + 1 }),
    }));
  },
  discard() {
    const { baseline, baselineDiagnostics } = get();
    set((state) => ({
      blueprint: baseline,
      diagnostics: baselineDiagnostics,
      dirty: false,
      editRevision: state.editRevision + 1,
      resourceRevision: state.resourceRevision + 1,
    }));
    return true;
  },
  hasErrors() { return get().diagnostics.some((d) => d.severity === 'error'); },
  reset() {
    set((state) => ({
      blueprint: null,
      baseline: null,
      baselineDiagnostics: [],
      path: null,
      diagnostics: [],
      dirty: false,
      editRevision: 0,
      resourceRevision: state.resourceRevision + 1,
      resourceIdentityRevision: state.resourceIdentityRevision + 1,
    }));
  },
}));
