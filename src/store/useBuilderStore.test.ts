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

  it('does not clear a newer edit that arrives while save is pending', async () => {
    const savedDraft = { schema: 2 as const, id: 'wf', name: 'First draft', nodes: [], edges: [] };
    const newerDraft = { ...savedDraft, name: 'Newer draft' };
    let finishSave: ((result: { written: boolean; diagnostics: [] }) => void) | undefined;
    invokeMock.mockReturnValueOnce(new Promise((resolve) => { finishSave = resolve; }));
    useBuilderStore.setState({
      blueprint: savedDraft,
      baseline: { ...savedDraft, name: 'Baseline' },
      path: '/x/wf.md',
      dirty: true,
      editRevision: 1,
    });

    const pendingSave = useBuilderStore.getState().save();
    useBuilderStore.getState().setBlueprint(newerDraft);
    finishSave?.({ written: true, diagnostics: [] });

    await expect(pendingSave).resolves.toBe(false);
    expect(useBuilderStore.getState().blueprint).toBe(newerDraft);
    expect(useBuilderStore.getState().baseline).toBe(savedDraft);
    expect(useBuilderStore.getState().dirty).toBe(true);
  });

  it('ignores a stale save response after another workflow resource loads', async () => {
    const first = { schema: 2 as const, id: 'one', name: 'One', nodes: [], edges: [] };
    const second = { schema: 2 as const, id: 'two', name: 'Two', nodes: [], edges: [] };
    let finishSave: ((result: { written: boolean; diagnostics: [] }) => void) | undefined;
    invokeMock.mockReturnValueOnce(new Promise((resolve) => { finishSave = resolve; }));
    useBuilderStore.setState({
      blueprint: first,
      baseline: first,
      baselineDiagnostics: [],
      path: '/x/one.md',
      dirty: true,
      editRevision: 1,
    });

    const pendingSave = useBuilderStore.getState().save();
    useBuilderStore.setState({
      blueprint: second,
      baseline: second,
      baselineDiagnostics: [{ severity: 'warning', code: 'two', message: 'two' }],
      path: '/x/two.md',
      diagnostics: [{ severity: 'warning', code: 'two', message: 'two' }],
      dirty: false,
      editRevision: 0,
    });
    finishSave?.({ written: true, diagnostics: [] });

    await expect(pendingSave).resolves.toBe(false);
    expect(useBuilderStore.getState()).toMatchObject({
      blueprint: second,
      baseline: second,
      path: '/x/two.md',
      dirty: false,
      diagnostics: [{ severity: 'warning', code: 'two', message: 'two' }],
    });
  });
});
