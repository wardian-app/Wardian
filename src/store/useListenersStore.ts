import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import type { AutomationListener, ListenerGatewayConfig, ListenerView } from '../types/automation';

let listenerLoadGeneration = 0;

interface ListenersState {
  listeners: ListenerView[];
  gateway: ListenerGatewayConfig | null;
  loading: boolean;
  error: string | null;
  load: () => Promise<void>;
  loadGateway: () => Promise<void>;
  subscribe: () => Promise<() => void>;
  save: (listener: AutomationListener) => Promise<ListenerView | null>;
  remove: (id: string) => Promise<void>;
  setEnabled: (id: string, enabled: boolean) => Promise<void>;
  /** Returns the secret exactly once; it is never readable afterwards. */
  rotateWebhookSecret: (id: string, secret?: string) => Promise<string | null>;
  setPollHeaders: (id: string, headers: Record<string, string>) => Promise<void>;
  saveGateway: (config: ListenerGatewayConfig) => Promise<void>;
}

export const useListenersStore = create<ListenersState>((set, get) => ({
  listeners: [],
  gateway: null,
  loading: false,
  error: null,

  async load() {
    const generation = ++listenerLoadGeneration;
    set({ loading: true, error: null });
    try {
      const listeners = await invoke<ListenerView[]>('listener_list');
      if (generation !== listenerLoadGeneration) return;
      const next = Array.isArray(listeners) ? listeners : [];
      // Listeners write runtime state on every fire, so an unchanged payload
      // is common; bailing out keeps a busy watcher from re-rendering the view.
      if (listenersEqual(get().listeners, next)) {
        set({ loading: false });
        return;
      }
      set({ listeners: next, loading: false });
    } catch (error) {
      if (generation !== listenerLoadGeneration) return;
      set({ error: String(error), loading: false });
    }
  },

  async loadGateway() {
    try {
      const gateway = await invoke<ListenerGatewayConfig>('listener_gateway_config');
      set({ gateway });
    } catch (error) {
      set({ error: String(error) });
    }
  },

  async subscribe() {
    return listen('listeners-updated', () => {
      void get().load();
    });
  },

  async save(listener) {
    try {
      const saved = await invoke<ListenerView>('listener_save', { listener });
      await get().load();
      return saved;
    } catch (error) {
      set({ error: String(error) });
      return null;
    }
  },

  async remove(id) {
    try {
      await invoke('listener_delete', { id });
      await get().load();
    } catch (error) {
      set({ error: String(error) });
    }
  },

  async setEnabled(id, enabled) {
    try {
      await invoke('listener_set_enabled', { id, enabled });
      await get().load();
    } catch (error) {
      set({ error: String(error) });
    }
  },

  async rotateWebhookSecret(id, secret) {
    try {
      const value = await invoke<string>('listener_set_webhook_secret', {
        id,
        ...(secret ? { secret } : {}),
      });
      await get().load();
      return value;
    } catch (error) {
      set({ error: String(error) });
      return null;
    }
  },

  async setPollHeaders(id, headers) {
    try {
      await invoke('listener_set_poll_headers', { id, headers });
      await get().load();
    } catch (error) {
      set({ error: String(error) });
    }
  },

  async saveGateway(config) {
    try {
      const saved = await invoke<ListenerGatewayConfig>('listener_gateway_save', { config });
      set({ gateway: saved });
      await get().load();
    } catch (error) {
      set({ error: String(error) });
    }
  },
}));

function listenersEqual(left: ListenerView[], right: ListenerView[]) {
  if (left.length !== right.length) return false;
  for (let index = 0; index < left.length; index += 1) {
    if (JSON.stringify(left[index]) !== JSON.stringify(right[index])) return false;
  }
  return true;
}
