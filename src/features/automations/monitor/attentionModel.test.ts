import { describe, expect, it } from 'vitest';
import { automationAttention } from './attentionModel';
import { buildMonitorModel } from './monitorModel';

describe('automationAttention', () => {
  it('keeps approval runs and unsuperseded failures in attention', () => {
    const result = automationAttention([
      { run_id: 'approval', blueprint_id: 'release', status: 'awaiting_approval', updated_at: '2026-08-31T12:00:00Z' },
      { run_id: 'failed', blueprint_id: 'audit', status: 'failed', updated_at: '2026-08-31T11:00:00Z' },
    ], []);

    expect([...result.runIds]).toEqual(['approval', 'failed']);
  });

  it('lets a newer completed or active run supersede an older failure', () => {
    const result = automationAttention([
      { run_id: 'failed', blueprint_id: 'audit', status: 'failed', updated_at: '2026-08-31T10:00:00Z' },
      { run_id: 'completed', blueprint_id: 'audit', status: 'completed', updated_at: '2026-08-31T11:00:00Z' },
      { run_id: 'older-failure', blueprint_id: 'release', status: 'failed', updated_at: '2026-08-31T10:00:00Z' },
      { run_id: 'running', blueprint_id: 'release', status: 'running', updated_at: '2026-08-31T12:00:00Z' },
    ], []);

    expect(result.runIds.size).toBe(0);
  });

  it('collapses listener runs by invoker the way it collapses schedule runs', () => {
    // Without this, a busy file listener's every fire would compete for
    // attention individually instead of being represented by its newest run.
    const result = automationAttention(
      [
        { run_id: 'old', blueprint_id: 'audit', listener_id: 'watcher', status: 'failed', updated_at: '2026-08-31T10:00:00Z' },
        { run_id: 'new', blueprint_id: 'audit', listener_id: 'watcher', status: 'completed', updated_at: '2026-08-31T12:00:00Z' },
      ],
      [],
    );

    expect(result.runIds.size).toBe(0);
  });

  it('keeps a listener failure in attention when nothing newer superseded it', () => {
    const result = automationAttention(
      [
        { run_id: 'failed', blueprint_id: 'audit', listener_id: 'watcher', status: 'failed', updated_at: '2026-08-31T12:00:00Z' },
      ],
      [],
    );

    expect([...result.runIds]).toEqual(['failed']);
  });

  it('retains schedule launch failures until a newer schedule run appears', () => {
    const failedAt = Date.parse('2026-08-31T11:00:00Z');
    const result = automationAttention([
      { run_id: 'new-run', blueprint_id: 'audit', schedule_id: 'recovered', status: 'running', updated_at: '2026-08-31T12:00:00Z' },
    ], [
      { id: 'unrecovered', last_run_status: 'failed', last_run_epoch_ms: failedAt },
      { id: 'recovered', last_run_status: 'failed', last_run_epoch_ms: failedAt },
      { id: 'paused', last_run_status: 'completed', last_run_epoch_ms: failedAt },
    ]);

    expect([...result.scheduleIds]).toEqual(['unrecovered']);
  });

  it('keeps a run visible when a temporary worker needs attention', () => {
    const result = automationAttention([{
      run_id: 'completed-with-worker',
      blueprint_id: 'audit',
      status: 'completed',
      updated_at: '2026-09-13T00:00:00Z',
      worker_attention_count: 1,
    }], []);

    expect([...result.runIds]).toEqual(['completed-with-worker']);
  });

  it('projects worker attention as its own monitor activity even after run completion', () => {
    const model = buildMonitorModel([{
      run_id: 'completed-with-worker',
      blueprint_id: 'audit',
      status: 'completed',
      node_count: 1,
      path: '/runs/completed-with-worker',
      worker_attention_count: 2,
    }], []);

    expect(model.activities[0]).toMatchObject({
      activityId: 'run:completed-with-worker',
      section: 'attention',
      statusLabel: 'Worker attention',
      issue: '2 temporary workers need attention',
    });
  });

  it('uses singular worker-attention grammar', () => {
    const model = buildMonitorModel([{
      run_id: 'completed-with-one-worker',
      blueprint_id: 'audit',
      status: 'completed',
      node_count: 1,
      path: '/runs/completed-with-one-worker',
      worker_attention_count: 1,
    }], []);

    expect(model.activities[0]?.issue).toBe('1 temporary worker needs attention');
  });
});
