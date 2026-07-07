import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { WorkflowDetail } from './WorkflowDetail';
import { LibraryEntry } from '../../../types';

const mockInvoke = vi.mocked(invoke);

function workflowEntry(overrides: Partial<LibraryEntry> = {}): LibraryEntry {
  return {
    kind: 'workflow',
    path: 'a/foo.md',
    entry_ref: 'workflows/a/foo.md',
    name: 'foo',
    description: '',
    tags: [],
    is_starred: false,
    deployment_count: 0,
    error: null,
    ...overrides,
  };
}

function renderWorkflowDetail(entry: LibraryEntry = workflowEntry()) {
  return render(
    <WorkflowDetail
      entry={entry}
      header={<div />}
      draft="# foo"
      dirty={false}
      stale={false}
      onChange={vi.fn()}
      onSave={vi.fn()}
      onReloadExternal={vi.fn()}
      onKeepMine={vi.fn()}
    />,
  );
}

// MINOR: blueprint resolution used to match via `path.endsWith(entryPath)`
// on raw strings, which false-positives whenever an absolute path happens
// to end in the same substring as the entry's relative path without a real
// segment boundary (e.g. `.../other-a/foo.md` "ends with" `a/foo.md`).
describe('WorkflowDetail blueprint resolution', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  it('resolves via an exact trailing-segment match, ignoring a colliding endsWith substring', async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'workflow_list_blueprints') {
        return [
          // Colliding path: string-wise `endsWith('a/foo.md')` would match
          // this too, even though its real leaf folder is `other-a`, not `a`.
          { id: 'collision', name: 'collision', path: 'C:/workspace/other-a/foo.md' },
          { id: 'correct', name: 'correct', path: 'C:/workspace/workflows/a/foo.md' },
        ];
      }
      if (cmd === 'workflow_parse') {
        return { blueprint: { schema: 1, id: 'correct', name: 'correct', nodes: [], edges: [] }, diagnostics: [] };
      }
      if (cmd === 'list_provider_readiness') return [];
      if (cmd === 'list_agents') return [];
      return null;
    });

    renderWorkflowDetail();

    fireEvent.click(screen.getByTestId('workflow-launch-run'));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('workflow_parse', { path: 'C:/workspace/workflows/a/foo.md' }),
    );
    expect(mockInvoke).not.toHaveBeenCalledWith('workflow_parse', { path: 'C:/workspace/other-a/foo.md' });
    expect(screen.queryByTestId('workflow-resolve-error')).not.toBeInTheDocument();
  });

  it('shows a resolve error when no ref has a real matching trailing segment', async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'workflow_list_blueprints') {
        return [{ id: 'collision', name: 'collision', path: 'C:/workspace/other-a/foo.md' }];
      }
      return null;
    });

    renderWorkflowDetail();

    fireEvent.click(screen.getByTestId('workflow-launch-run'));

    expect(await screen.findByTestId('workflow-resolve-error')).toHaveTextContent(
      'Could not locate this workflow file on disk.',
    );
    expect(mockInvoke).not.toHaveBeenCalledWith('workflow_parse', expect.anything());
  });
});
