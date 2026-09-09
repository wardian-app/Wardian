import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ListenerView } from '../types/automation';
import { useListenersStore } from './useListenersStore';

const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invokeMock(...args) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));

const listener = (id: string, agentId: string): ListenerView => ({
  id,
  blueprint_id: id,
  name: id,
  enabled: true,
  trigger: {
    type: 'file_watch',
    path: '/workspace',
    recursive: true,
    patterns: [],
    ignore: [],
    events: ['created'],
    debounce_ms: 250,
  },
  input: {},
  bindings: {},
  assignments: {
    worker: { target_type: 'agent', agent_id: agentId, conversation: 'current' },
  },
  runtime: { armed: true, fire_count: 0, recent_fire_epoch_ms: [], consecutive_failures: 0 },
  has_secret: false,
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((next) => {
    resolve = next;
  });
  return { promise, resolve };
}

beforeEach(() => {
  invokeMock.mockReset();
  useListenersStore.setState({ listeners: [], gateway: null, loading: false, error: null });
});

describe('useListenersStore', () => {
  it('does not let an older listener snapshot overwrite a newer assignment', async () => {
    const older = deferred<ListenerView[]>();
    const newer = deferred<ListenerView[]>();
    invokeMock
      .mockReturnValueOnce(older.promise)
      .mockReturnValueOnce(newer.promise);

    const olderLoad = useListenersStore.getState().load();
    const newerLoad = useListenersStore.getState().load();
    newer.resolve([listener('newer', 'agent-new')]);
    await newerLoad;

    expect(useListenersStore.getState().listeners).toEqual([listener('newer', 'agent-new')]);

    older.resolve([listener('older', 'agent-old')]);
    await olderLoad;

    expect(useListenersStore.getState().listeners).toEqual([listener('newer', 'agent-new')]);
    expect(useListenersStore.getState().loading).toBe(false);
  });
});
