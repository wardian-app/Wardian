import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeMock = vi.hoisted(() => vi.fn());
const listenMock = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invokeMock(...a) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: (...a: unknown[]) => listenMock(...a) }));

import { useSchedulesStore } from './useSchedulesStore';

const sample = {
  id: 's1',
  blueprint_id: 'heartbeat',
  name: 'HB',
  input: {},
  bindings: {},
  schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
  is_paused: false,
};

describe('useSchedulesStore', () => {
  beforeEach(() => {
    invokeMock.mockReset();
    listenMock.mockReset();
    listenMock.mockResolvedValue(() => {});
    useSchedulesStore.setState({ schedules: [], loading: false, error: null });
  });

  it('load populates schedules from schedule_list_v2', async () => {
    invokeMock.mockResolvedValueOnce([sample]);
    await useSchedulesStore.getState().load();
    expect(invokeMock).toHaveBeenCalledWith('schedule_list_v2');
    expect(useSchedulesStore.getState().schedules).toHaveLength(1);
  });

  it('pause invokes schedule_pause_v2 with the id', async () => {
    invokeMock.mockResolvedValue([]);
    await useSchedulesStore.getState().pause('s1');
    expect(invokeMock).toHaveBeenCalledWith('schedule_pause_v2', { id: 's1' });
  });

  it('runNow invokes schedule_run_now_v2 with the id', async () => {
    invokeMock.mockResolvedValue([]);
    await useSchedulesStore.getState().runNow('s1');
    expect(invokeMock).toHaveBeenCalledWith('schedule_run_now_v2', { id: 's1' });
  });

  it('create invokes schedule_create_v2 with camelCase args', async () => {
    invokeMock.mockResolvedValue(sample);
    await useSchedulesStore.getState().create({
      blueprintId: 'heartbeat',
      name: 'HB',
      schedule: { schedule_type: 'interval', interval_minutes: 60, active: true },
      provider: 'codex',
      input: {},
      bindings: {},
    });
    expect(invokeMock).toHaveBeenCalledWith(
      'schedule_create_v2',
      expect.objectContaining({ blueprintId: 'heartbeat', name: 'HB' }),
    );
  });

  it('subscribe registers a v2-schedules-updated listener', async () => {
    await useSchedulesStore.getState().subscribe();
    expect(listenMock).toHaveBeenCalledWith('v2-schedules-updated', expect.any(Function));
  });
});
