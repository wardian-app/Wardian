import { describe, it, expect, vi, beforeEach } from 'vitest';

const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invokeMock(...a) }));

import { useBuilderStore } from './useBuilderStore';

describe('useBuilderStore', () => {
  beforeEach(() => { invokeMock.mockReset(); useBuilderStore.getState().reset(); });

  it('loads a blueprint via workflow_parse', async () => {
    invokeMock.mockResolvedValueOnce({ blueprint: { schema: 2, id: 'wf', name: 'WF', nodes: [], edges: [] }, diagnostics: [] });
    await useBuilderStore.getState().load('/x/wf.md');
    expect(invokeMock).toHaveBeenCalledWith('workflow_parse', { path: '/x/wf.md' });
    expect(useBuilderStore.getState().blueprint?.id).toBe('wf');
  });

  it('stores diagnostics from validate and blocks save when invalid', async () => {
    useBuilderStore.setState({ blueprint: { schema: 2, id: 'wf', name: 'WF', nodes: [], edges: [] } });
    invokeMock.mockResolvedValueOnce({ ok: false, diagnostics: [{ severity: 'error', code: 'x', message: 'bad', node: 'n1' }] });
    await useBuilderStore.getState().validate();
    expect(useBuilderStore.getState().diagnostics).toHaveLength(1);
    expect(useBuilderStore.getState().hasErrors()).toBe(true);
  });
});
