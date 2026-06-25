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

  it('load populates schedules from schedule_list', async () => {
    invokeMock.mockResolvedValueOnce([sample]);
    await useSchedulesStore.getState().load();
    expect(invokeMock).toHaveBeenCalledWith('schedule_list');
    expect(useSchedulesStore.getState().schedules).toHaveLength(1);
  });

  it('keeps the existing schedule array when polling returns unchanged schedules', async () => {
    invokeMock
      .mockResolvedValueOnce([sample])
      .mockResolvedValueOnce([{ ...sample }]);

    await useSchedulesStore.getState().load();
    const firstSchedules = useSchedulesStore.getState().schedules;
    await useSchedulesStore.getState().load();

    expect(useSchedulesStore.getState().schedules).toBe(firstSchedules);
  });

  it('pause invokes schedule_pause with the id', async () => {
    invokeMock.mockResolvedValue([]);
    await useSchedulesStore.getState().pause('s1');
    expect(invokeMock).toHaveBeenCalledWith('schedule_pause', { id: 's1' });
  });

  it('runNow invokes schedule_run_now with the id', async () => {
    invokeMock.mockResolvedValue([]);
    await useSchedulesStore.getState().runNow('s1');
    expect(invokeMock).toHaveBeenCalledWith('schedule_run_now', { id: 's1' });
  });

  it('create invokes schedule_create with camelCase args', async () => {
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
      'schedule_create',
      expect.objectContaining({ blueprintId: 'heartbeat', name: 'HB' }),
    );
  });

  it('subscribe registers a schedules-updated listener', async () => {
    await useSchedulesStore.getState().subscribe();
    expect(listenMock).toHaveBeenCalledWith('schedules-updated', expect.any(Function));
  });
});
