import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import type { ScheduleDefinition, WorkflowSchedule } from '../types/workflow';

export interface CreateScheduleArgs {
  blueprintId: string;
  name: string;
  schedule: ScheduleDefinition;
  provider?: string;
  workspace?: string;
  input?: unknown;
  bindings?: Record<string, string>;
}

interface SchedulesState {
  schedules: WorkflowSchedule[];
  loading: boolean;
  error: string | null;
  load: () => Promise<void>;
  subscribe: () => Promise<() => void>;
  create: (args: CreateScheduleArgs) => Promise<void>;
  pause: (id: string) => Promise<void>;
  resume: (id: string) => Promise<void>;
  remove: (id: string) => Promise<void>;
  runNow: (id: string) => Promise<void>;
}

export const useSchedulesStore = create<SchedulesState>((set, get) => ({
  schedules: [],
  loading: false,
  error: null,

  async load() {
    set({ loading: true, error: null });
    try {
      const schedules = await invoke<WorkflowSchedule[]>('schedule_list_v2');
      set({ schedules: Array.isArray(schedules) ? schedules : [], loading: false });
    } catch (error) {
      set({ error: String(error), loading: false });
    }
  },

  async subscribe() {
    return listen('v2-schedules-updated', () => {
      void get().load();
    });
  },

  async create(args) {
    try {
      await invoke('schedule_create_v2', {
        blueprintId: args.blueprintId,
        name: args.name,
        schedule: args.schedule,
        ...(args.provider ? { provider: args.provider } : {}),
        ...(args.workspace ? { workspace: args.workspace } : {}),
        ...(args.input !== undefined ? { input: args.input } : {}),
        ...(args.bindings ? { bindings: args.bindings } : {}),
      });
      await get().load();
    } catch (error) {
      set({ error: String(error) });
    }
  },

  async pause(id) {
    try {
      await invoke('schedule_pause_v2', { id });
      await get().load();
    } catch (error) {
      set({ error: String(error) });
    }
  },

  async resume(id) {
    try {
      await invoke('schedule_resume_v2', { id });
      await get().load();
    } catch (error) {
      set({ error: String(error) });
    }
  },

  async remove(id) {
    try {
      await invoke('schedule_remove_v2', { id });
      await get().load();
    } catch (error) {
      set({ error: String(error) });
    }
  },

  async runNow(id) {
    try {
      await invoke('schedule_run_now_v2', { id });
      await get().load();
    } catch (error) {
      set({ error: String(error) });
    }
  },
}));
