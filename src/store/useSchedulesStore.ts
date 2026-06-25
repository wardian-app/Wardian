import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import type { ScheduleDefinition, WorkflowAssignments, WorkflowSchedule } from '../types/workflow';

export interface CreateScheduleArgs {
  blueprintId: string;
  name: string;
  schedule: ScheduleDefinition;
  provider?: string;
  workspace?: string;
  input?: unknown;
  bindings?: Record<string, string>;
  assignments?: WorkflowAssignments;
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
      const schedules = await invoke<WorkflowSchedule[]>('schedule_list');
      const nextSchedules = Array.isArray(schedules) ? schedules : [];
      if (workflowSchedulesEqual(get().schedules, nextSchedules)) {
        set({ loading: false });
        return;
      }
      set({ schedules: nextSchedules, loading: false });
    } catch (error) {
      set({ error: String(error), loading: false });
    }
  },

  async subscribe() {
    return listen('schedules-updated', () => {
      void get().load();
    });
  },

  async create(args) {
    try {
      await invoke('schedule_create', {
        blueprintId: args.blueprintId,
        name: args.name,
        schedule: args.schedule,
        ...(args.provider ? { provider: args.provider } : {}),
        ...(args.workspace ? { workspace: args.workspace } : {}),
        ...(args.input !== undefined ? { input: args.input } : {}),
        ...(args.bindings ? { bindings: args.bindings } : {}),
        ...(args.assignments ? { assignments: args.assignments } : {}),
      });
      await get().load();
    } catch (error) {
      set({ error: String(error) });
    }
  },

  async pause(id) {
    try {
      await invoke('schedule_pause', { id });
      await get().load();
    } catch (error) {
      set({ error: String(error) });
    }
  },

  async resume(id) {
    try {
      await invoke('schedule_resume', { id });
      await get().load();
    } catch (error) {
      set({ error: String(error) });
    }
  },

  async remove(id) {
    try {
      await invoke('schedule_remove', { id });
      await get().load();
    } catch (error) {
      set({ error: String(error) });
    }
  },

  async runNow(id) {
    try {
      await invoke('schedule_run_now', { id });
      await get().load();
    } catch (error) {
      set({ error: String(error) });
    }
  },
}));

function workflowSchedulesEqual(left: WorkflowSchedule[], right: WorkflowSchedule[]) {
  if (left.length !== right.length) return false;
  for (let index = 0; index < left.length; index += 1) {
    if (workflowScheduleSignature(left[index]) !== workflowScheduleSignature(right[index])) return false;
  }
  return true;
}

function workflowScheduleSignature(schedule: WorkflowSchedule) {
  return JSON.stringify(schedule);
}
