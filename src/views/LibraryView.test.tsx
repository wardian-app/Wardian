import { act, fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { LibraryView } from './LibraryView';
import { useLibraryStore } from '../store/useLibraryStore';
import { LibraryIndex } from '../types';

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

describe('LibraryView', () => {
  beforeEach(() => {
    useLibraryStore.setState({
      index: emptyIndex,
      isLoading: false,
      error: null,
      activeSection: 'skills',
    });
  });

  it('renders the section rail, list, and detail regions', () => {
    render(<LibraryView selectedAgentIds={new Set()} />);

    expect(screen.getByTestId('library-view')).toBeInTheDocument();
    expect(screen.getByTestId('library-section-rail')).toBeInTheDocument();
    expect(screen.getByTestId('library-list')).toBeInTheDocument();
    expect(screen.getByTestId('library-detail')).toBeInTheDocument();
  });

  it('subscribes to library changes on mount', () => {
    const cleanup = vi.fn();
    const subscribeToLibraryChanges = vi.fn(() => cleanup);
    useLibraryStore.setState({ subscribeToLibraryChanges });

    const { unmount } = render(<LibraryView selectedAgentIds={new Set()} />);

    expect(subscribeToLibraryChanges).toHaveBeenCalledTimes(1);

    unmount();
    expect(cleanup).toHaveBeenCalledTimes(1);
  });

  it('switching sections updates the store activeSection', () => {
    render(<LibraryView selectedAgentIds={new Set()} />);

    expect(useLibraryStore.getState().activeSection).toBe('skills');

    act(() => {
      fireEvent.click(screen.getByTestId('library-section-prompts'));
    });

    expect(useLibraryStore.getState().activeSection).toBe('prompts');
  });
});
