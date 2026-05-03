import { act, render } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { LibraryView } from './LibraryView';
import { useLibraryStore } from '../store/useLibraryStore';

vi.mock('../features/library/LibraryGrid', () => ({
  LibraryGrid: () => <div data-testid="library-grid" />,
}));

describe('LibraryView', () => {
  beforeEach(() => {
    useLibraryStore.setState({
      promptTree: { type: 'Folder', path: '', name: 'prompts', children: [] },
      skillTree: { type: 'Folder', path: '', name: 'skills', children: [] },
      isLoading: false,
      error: null,
      activeTab: 'skills',
    });
  });

  it('subscribes to skill library changes only while the skills tab is active', () => {
    const cleanup = vi.fn();
    const subscribeToLibraryChanges = vi.fn(() => cleanup);
    useLibraryStore.setState({ subscribeToLibraryChanges });

    render(<LibraryView selectedAgentIds={new Set()} />);

    expect(subscribeToLibraryChanges).toHaveBeenCalledWith('skills');

    act(() => {
      useLibraryStore.setState({ activeTab: 'prompts' });
    });

    expect(cleanup).toHaveBeenCalledTimes(1);
  });
});
