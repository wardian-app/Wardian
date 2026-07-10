import { describe, it, expect, vi, beforeEach } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useLibraryStore } from './useLibraryStore';
import { LibraryIndex } from '../types';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));

const mockListen = vi.mocked(listen);

async function flushPromises() {
  await Promise.resolve();
  await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
}

const emptyIndex: LibraryIndex = {
  sections: {
    skills: { tree: { path: '', name: 'Root', children: [] }, stubbed: false },
    prompts: { tree: { path: '', name: 'Root', children: [] }, stubbed: false },
    workflows: { tree: { path: '', name: 'Root', children: [] }, stubbed: false },
    classes: { tree: { path: '', name: 'Root', children: [] }, stubbed: false },
    mcps: { tree: { path: '', name: 'Root', children: [] }, stubbed: true },
  },
  deployments: {},
  orphans: [],
};

describe('useLibraryStore', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    mockListen.mockReset();
    mockListen.mockResolvedValue(() => {});
    useLibraryStore.setState({
      index: null,
      selection: null,
      selectedContent: null,
      contentStale: false,
      isLoading: false,
      error: null,
      _editorDirty: false,
      libraryDetailWidth: 480,
    });
  });

  it('owns and clamps its transient detail-pane width', () => {
    Object.defineProperty(window, 'innerWidth', { value: 1_000, configurable: true });
    useLibraryStore.getState().setLibraryDetailWidth(100);
    expect(useLibraryStore.getState().libraryDetailWidth).toBe(360);
    useLibraryStore.getState().setLibraryDetailWidth(900);
    expect(useLibraryStore.getState().libraryDetailWidth).toBe(700);
    useLibraryStore.getState().resetLibraryDetailWidth();
    expect(useLibraryStore.getState().libraryDetailWidth).toBe(480);
  });

  it('fetchIndex loads the unified index', async () => {
    vi.mocked(invoke).mockResolvedValueOnce(emptyIndex);
    await useLibraryStore.getState().fetchIndex();
    expect(invoke).toHaveBeenCalledWith('get_library_index');
    expect(useLibraryStore.getState().index?.sections.mcps.stubbed).toBe(true);
  });

  it('fetchIndex records the error message on failure', async () => {
    vi.mocked(invoke).mockRejectedValueOnce(new Error('boom'));
    await useLibraryStore.getState().fetchIndex();
    expect(useLibraryStore.getState().error).toBe('boom');
    expect(useLibraryStore.getState().isLoading).toBe(false);
  });

  it('select lazy-loads content', async () => {
    vi.mocked(invoke).mockResolvedValueOnce('# body');
    await useLibraryStore.getState().select('skills/dev/planner');
    expect(invoke).toHaveBeenCalledWith('read_library_item', { section: 'skills', path: 'dev/planner' });
    expect(useLibraryStore.getState().selectedContent).toBe('# body');
    expect(useLibraryStore.getState().selection).toEqual({ section: 'skills', entryRef: 'skills/dev/planner' });
  });

  it('select(null) clears the current selection', async () => {
    vi.mocked(invoke).mockResolvedValueOnce('# body');
    await useLibraryStore.getState().select('skills/dev/planner');
    await useLibraryStore.getState().select(null);
    expect(useLibraryStore.getState().selection).toBeNull();
    expect(useLibraryStore.getState().selectedContent).toBeNull();
  });

  it('saveItem invokes save_library_item with snake_case args and refreshes the index', async () => {
    vi.mocked(invoke).mockResolvedValueOnce(undefined).mockResolvedValueOnce(emptyIndex);
    await useLibraryStore.getState().saveItem('skills', 'dev/planner', 'body');
    expect(invoke).toHaveBeenNthCalledWith(1, 'save_library_item', {
      section: 'skills',
      path: 'dev/planner',
      content: 'body',
    });
    expect(invoke).toHaveBeenNthCalledWith(2, 'get_library_index');
  });

  it('updateMetadata invokes update_library_metadata and refreshes the index', async () => {
    const metadata = { id: 'uuid-1', tags: ['dev'], is_starred: true };
    vi.mocked(invoke).mockResolvedValueOnce(undefined).mockResolvedValueOnce(emptyIndex);
    await useLibraryStore.getState().updateMetadata('skills/dev/planner', metadata);
    expect(invoke).toHaveBeenNthCalledWith(1, 'update_library_metadata', {
      entryRef: 'skills/dev/planner',
      metadata,
    });
    expect(invoke).toHaveBeenNthCalledWith(2, 'get_library_index');
  });

  it('createFolder invokes create_library_folder and refreshes the index', async () => {
    vi.mocked(invoke).mockResolvedValueOnce(undefined).mockResolvedValueOnce(emptyIndex);
    await useLibraryStore.getState().createFolder('prompts', 'dev/new-folder');
    expect(invoke).toHaveBeenNthCalledWith(1, 'create_library_folder', {
      section: 'prompts',
      path: 'dev/new-folder',
    });
    expect(invoke).toHaveBeenNthCalledWith(2, 'get_library_index');
  });

  it('renameEntry invokes rename_library_entry and refreshes the index', async () => {
    vi.mocked(invoke).mockResolvedValueOnce(undefined).mockResolvedValueOnce(emptyIndex);
    await useLibraryStore.getState().renameEntry('skills', 'dev/planner', 'dev/planner-2');
    expect(invoke).toHaveBeenNthCalledWith(1, 'rename_library_entry', {
      section: 'skills',
      fromPath: 'dev/planner',
      toPath: 'dev/planner-2',
    });
    expect(invoke).toHaveBeenNthCalledWith(2, 'get_library_index');
  });

  it('deleteEntry invokes delete_library_entry and refreshes the index', async () => {
    vi.mocked(invoke).mockResolvedValueOnce(undefined).mockResolvedValueOnce(emptyIndex);
    await useLibraryStore.getState().deleteEntry('skills', 'dev/planner');
    expect(invoke).toHaveBeenNthCalledWith(1, 'delete_library_entry', {
      section: 'skills',
      path: 'dev/planner',
    });
    expect(invoke).toHaveBeenNthCalledWith(2, 'get_library_index');
  });

  it('setSkillDeployments invokes command and refreshes index', async () => {
    vi.mocked(invoke).mockResolvedValue(emptyIndex);
    await useLibraryStore.getState().setSkillDeployments('dev/planner', [{ target_type: 'class', target_id: 'Architect' }]);
    expect(invoke).toHaveBeenCalledWith('set_skill_deployments', {
      sourcePath: 'dev/planner',
      targets: [{ target_type: 'class', target_id: 'Architect' }],
    });
  });

  it('removeOrphan invokes remove_orphan_deployment and refreshes the index', async () => {
    vi.mocked(invoke).mockResolvedValueOnce(undefined).mockResolvedValueOnce(emptyIndex);
    await useLibraryStore.getState().removeOrphan({ target_type: 'agent', target_id: 'agent-1', skill_name: 'planner' });
    expect(invoke).toHaveBeenNthCalledWith(1, 'remove_orphan_deployment', {
      targetType: 'agent',
      targetId: 'agent-1',
      skillName: 'planner',
    });
    expect(invoke).toHaveBeenNthCalledWith(2, 'get_library_index');
  });

  it('markEditorDirty sets _editorDirty without touching selectedContent', () => {
    useLibraryStore.getState().markEditorDirty(true);
    expect(useLibraryStore.getState()._editorDirty).toBe(true);
    expect(invoke).not.toHaveBeenCalled();
    useLibraryStore.getState().markEditorDirty(false);
    expect(useLibraryStore.getState()._editorDirty).toBe(false);
  });

  describe('mutation error handling', () => {
    it('saveItem sets error state and rejects on failure', async () => {
      vi.mocked(invoke).mockRejectedValueOnce(new Error('save failed'));
      await expect(
        useLibraryStore.getState().saveItem('skills', 'dev/planner', 'body'),
      ).rejects.toThrow('save failed');
      expect(useLibraryStore.getState().error).toBe('save failed');
    });

    it('updateMetadata sets error state and rejects on failure', async () => {
      vi.mocked(invoke).mockRejectedValueOnce(new Error('update failed'));
      await expect(
        useLibraryStore.getState().updateMetadata('skills/dev/planner', {
          id: 'uuid-1',
          tags: [],
          is_starred: false,
        }),
      ).rejects.toThrow('update failed');
      expect(useLibraryStore.getState().error).toBe('update failed');
    });

    it('createFolder sets error state and rejects on failure', async () => {
      vi.mocked(invoke).mockRejectedValueOnce(new Error('create failed'));
      await expect(
        useLibraryStore.getState().createFolder('prompts', 'dev/new-folder'),
      ).rejects.toThrow('create failed');
      expect(useLibraryStore.getState().error).toBe('create failed');
    });

    it('renameEntry sets error state and rejects on failure', async () => {
      vi.mocked(invoke).mockRejectedValueOnce(new Error('rename failed'));
      await expect(
        useLibraryStore.getState().renameEntry('skills', 'dev/planner', 'dev/planner-2'),
      ).rejects.toThrow('rename failed');
      expect(useLibraryStore.getState().error).toBe('rename failed');
    });

    it('deleteEntry sets error state and rejects on failure', async () => {
      vi.mocked(invoke).mockRejectedValueOnce(new Error('delete failed'));
      await expect(
        useLibraryStore.getState().deleteEntry('skills', 'dev/planner'),
      ).rejects.toThrow('delete failed');
      expect(useLibraryStore.getState().error).toBe('delete failed');
    });

    it('setSkillDeployments sets error state and rejects on failure', async () => {
      vi.mocked(invoke).mockRejectedValueOnce(new Error('deploy failed'));
      await expect(
        useLibraryStore
          .getState()
          .setSkillDeployments('dev/planner', [{ target_type: 'class', target_id: 'Architect' }]),
      ).rejects.toThrow('deploy failed');
      expect(useLibraryStore.getState().error).toBe('deploy failed');
    });

    it('removeOrphan sets error state and rejects on failure', async () => {
      vi.mocked(invoke).mockRejectedValueOnce(new Error('remove failed'));
      await expect(
        useLibraryStore
          .getState()
          .removeOrphan({ target_type: 'agent', target_id: 'agent-1', skill_name: 'planner' }),
      ).rejects.toThrow('remove failed');
      expect(useLibraryStore.getState().error).toBe('remove failed');
    });

    it('does not refetch the index when the mutation itself fails', async () => {
      vi.mocked(invoke).mockRejectedValueOnce(new Error('save failed'));
      await expect(
        useLibraryStore.getState().saveItem('skills', 'dev/planner', 'body'),
      ).rejects.toThrow('save failed');
      expect(invoke).toHaveBeenCalledTimes(1);
    });

    // renameEntry/deleteEntry/setSkillDeployments wrap core mutations that
    // are deliberately best-effort: disk can have partially changed even
    // though the call rejected (e.g. a rename's source move + metadata
    // migration succeed but a re-link to one deployment target fails), and
    // the watcher doesn't cover agents/*  or common/ deployment directories
    // to rescue the stale index another way. Unlike saveItem/updateMetadata/
    // createFolder (which fail atomically with nothing on disk to refetch),
    // these three must refetch on failure too (final-review FIX-NOW 3).
    describe('best-effort mutations refetch the index even when they fail', () => {
      it('renameEntry refetches on failure', async () => {
        vi.mocked(invoke).mockRejectedValueOnce(new Error('rename failed')).mockResolvedValueOnce(emptyIndex);
        await expect(
          useLibraryStore.getState().renameEntry('skills', 'dev/planner', 'dev/planner-2'),
        ).rejects.toThrow('rename failed');
        expect(invoke).toHaveBeenCalledTimes(2);
        expect(invoke).toHaveBeenNthCalledWith(2, 'get_library_index');
        expect(useLibraryStore.getState().error).toBe('rename failed');
        expect(useLibraryStore.getState().index).toEqual(emptyIndex);
      });

      it('deleteEntry refetches on failure', async () => {
        vi.mocked(invoke).mockRejectedValueOnce(new Error('delete failed')).mockResolvedValueOnce(emptyIndex);
        await expect(
          useLibraryStore.getState().deleteEntry('skills', 'dev/planner'),
        ).rejects.toThrow('delete failed');
        expect(invoke).toHaveBeenCalledTimes(2);
        expect(invoke).toHaveBeenNthCalledWith(2, 'get_library_index');
        expect(useLibraryStore.getState().error).toBe('delete failed');
        expect(useLibraryStore.getState().index).toEqual(emptyIndex);
      });

      it('setSkillDeployments refetches on failure', async () => {
        vi.mocked(invoke).mockRejectedValueOnce(new Error('deploy failed')).mockResolvedValueOnce(emptyIndex);
        await expect(
          useLibraryStore
            .getState()
            .setSkillDeployments('dev/planner', [{ target_type: 'class', target_id: 'Architect' }]),
        ).rejects.toThrow('deploy failed');
        expect(invoke).toHaveBeenCalledTimes(2);
        expect(invoke).toHaveBeenNthCalledWith(2, 'get_library_index');
        expect(useLibraryStore.getState().error).toBe('deploy failed');
        expect(useLibraryStore.getState().index).toEqual(emptyIndex);
      });
    });
  });

  describe('renameEntry follows the current selection to its new ref', () => {
    it('re-selects the renamed entry under its new entry_ref when it was selected', async () => {
      vi.mocked(invoke)
        .mockResolvedValueOnce(undefined) // rename_library_entry
        .mockResolvedValueOnce(emptyIndex) // get_library_index (fetchIndex)
        .mockResolvedValueOnce('# body'); // read_library_item (select)
      useLibraryStore.setState({ selection: { section: 'skills', entryRef: 'skills/dev/planner' } });

      await useLibraryStore.getState().renameEntry('skills', 'dev/planner', 'dev/planner-2');

      expect(useLibraryStore.getState().selection).toEqual({
        section: 'skills',
        entryRef: 'skills/dev/planner-2',
      });
      expect(invoke).toHaveBeenNthCalledWith(3, 'read_library_item', { section: 'skills', path: 'dev/planner-2' });
      expect(useLibraryStore.getState().selectedContent).toBe('# body');
    });

    it('leaves selection untouched when a different entry is selected', async () => {
      vi.mocked(invoke).mockResolvedValueOnce(undefined).mockResolvedValueOnce(emptyIndex);
      useLibraryStore.setState({ selection: { section: 'skills', entryRef: 'skills/other' } });

      await useLibraryStore.getState().renameEntry('skills', 'dev/planner', 'dev/planner-2');

      expect(useLibraryStore.getState().selection).toEqual({ section: 'skills', entryRef: 'skills/other' });
      expect(invoke).toHaveBeenCalledTimes(2);
    });
  });

  it('external change with dirty editor sets contentStale instead of reloading content', async () => {
    let handler: ((event: { payload: { library_type: string } }) => void) | undefined;
    mockListen.mockImplementation(async (_event, cb) => {
      handler = cb as typeof handler;
      return () => {};
    });
    vi.mocked(invoke).mockResolvedValue(emptyIndex);

    const cleanup = useLibraryStore.getState().subscribeToLibraryChanges();
    await flushPromises();

    vi.mocked(invoke).mockResolvedValueOnce('# original');
    await useLibraryStore.getState().select('skills/dev/planner', { editorDirty: true });
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue(emptyIndex);

    handler?.({ payload: { library_type: 'library' } });
    await flushPromises();

    expect(useLibraryStore.getState().contentStale).toBe(true);
    expect(useLibraryStore.getState().selectedContent).toBe('# original');
    expect(invoke).not.toHaveBeenCalledWith('read_library_item', expect.anything());

    cleanup();
  });

  it('external change with a clean editor reloads the selected content', async () => {
    let handler: ((event: { payload: { library_type: string } }) => void) | undefined;
    mockListen.mockImplementation(async (_event, cb) => {
      handler = cb as typeof handler;
      return () => {};
    });
    vi.mocked(invoke).mockResolvedValue(emptyIndex);

    const cleanup = useLibraryStore.getState().subscribeToLibraryChanges();
    await flushPromises();

    vi.mocked(invoke).mockResolvedValueOnce('# original');
    await useLibraryStore.getState().select('skills/dev/planner');
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValueOnce(emptyIndex).mockResolvedValueOnce('# updated');

    handler?.({ payload: { library_type: 'library' } });
    await flushPromises();

    expect(useLibraryStore.getState().contentStale).toBe(false);
    expect(useLibraryStore.getState().selectedContent).toBe('# updated');
    expect(invoke).toHaveBeenCalledWith('read_library_item', { section: 'skills', path: 'dev/planner' });

    cleanup();
  });
});
