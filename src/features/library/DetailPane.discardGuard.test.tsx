import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { DetailPane } from './DetailPane';
import { useLibraryStore } from '../../store/useLibraryStore';
import { LibraryEntry, LibraryIndex } from '../../types';

const mockInvoke = vi.mocked(invoke);

function entry(overrides: Partial<LibraryEntry> & Pick<LibraryEntry, 'kind' | 'name' | 'path' | 'entry_ref'>): LibraryEntry {
  return {
    description: '',
    tags: [],
    is_starred: false,
    deployment_count: 0,
    error: null,
    ...overrides,
  };
}

const emptyTree = { path: '', name: 'Root', children: [] };

function buildIndex(): LibraryIndex {
  return {
    sections: {
      skills: {
        stubbed: false,
        tree: {
          path: '',
          name: 'Root',
          children: [
            entry({ kind: 'skill', name: 'alpha', path: 'alpha', entry_ref: 'skills/alpha' }),
            entry({ kind: 'skill', name: 'beta', path: 'beta', entry_ref: 'skills/beta' }),
          ],
        },
      },
      prompts: { stubbed: false, tree: emptyTree },
      workflows: { stubbed: false, tree: emptyTree },
      classes: { stubbed: false, tree: emptyTree },
      mcps: { stubbed: true, tree: emptyTree },
    },
    deployments: {},
    orphans: [],
  };
}

/**
 * Regression coverage for Task 15 review Critical #1: the "Discard changes?"
 * guard's Cancel path used to call `store.select(previousRef, { editorDirty:
 * true })`, which unconditionally re-reads the file from disk and updates
 * `selectedContent` — the content-adopt effect then overwrote the local
 * dirty draft with that disk content, silently discarding the exact edits
 * the user chose to keep.
 *
 * This suite deliberately drives a REAL `useLibraryStore.select()` round
 * trip (only the Tauri `invoke` boundary is mocked) instead of the
 * `useLibraryStore.setState(...)` shortcut `DetailPane.test.tsx` uses for
 * its other cases — the review noted the bug only reproduces through a
 * genuine `select()` call combined with a locally-tracked dirty draft, which
 * a fully-mocked store papers over.
 */
describe('DetailPane — discard-confirm Cancel preserves the dirty draft (regression)', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    // Merge-only reset (no `replace`) so the store keeps its real action
    // closures (`select`, `revertSelection`, ...) instead of being wiped —
    // this test needs the genuine implementations, not mocks.
    useLibraryStore.setState({
      index: buildIndex(),
      activeSection: 'skills',
      selection: null,
      selectedContent: null,
      contentStale: false,
      _editorDirty: false,
      _editorResources: {},
    });
  });

  it('keeps the edited draft when the user cancels the discard-changes prompt', async () => {
    mockInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === 'read_library_item') {
        const path = (args as { path: string }).path;
        if (path === 'alpha') return '# Alpha original';
        if (path === 'beta') return '# Beta original';
      }
      return null;
    });

    // Initial load: select the entry that will end up dirty.
    await act(async () => {
      await useLibraryStore.getState().select('skills/alpha');
    });

    render(<DetailPane selectedAgentIds={new Set()} />);

    const textarea = await screen.findByTestId('markdown-editor-textarea');
    expect(textarea).toHaveValue('# Alpha original');

    // Edit — this is the draft the user wants to KEEP.
    fireEvent.change(textarea, { target: { value: '# Alpha original + my edits' } });
    expect(textarea).toHaveValue('# Alpha original + my edits');

    // Attempt switch: simulate LibraryList's click handler calling the
    // store's real `select()` directly (DetailPane doesn't intercept
    // LibraryList clicks — this is the normal, expected path). This is a
    // genuine disk read for `beta`, exactly like production.
    await act(async () => {
      await useLibraryStore.getState().select('skills/beta');
    });

    // No ConfirmProvider is mounted, so useConfirm()'s default context value
    // (`async () => false`) resolves the "Discard changes?" prompt as
    // Cancel — this exercises the decline path under test. Wait for the
    // guard effect's `.then()` to run and revert the selection.
    await waitFor(() => expect(useLibraryStore.getState().selection?.entryRef).toBe('skills/alpha'));

    // The crux of the regression: the draft must still contain the user's
    // edits, not the disk content of `alpha` or the disk content fetched for
    // the declined `beta` switch.
    expect(screen.getByTestId('markdown-editor-textarea')).toHaveValue('# Alpha original + my edits');

    // The revert must not have re-read `alpha` from disk a second time —
    // that's the whole point of `revertSelection` over `select()`.
    const alphaReads = mockInvoke.mock.calls.filter(
      ([cmd, args]) => cmd === 'read_library_item' && (args as { path?: string } | undefined)?.path === 'alpha',
    );
    expect(alphaReads).toHaveLength(1);
  });
});
