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
    act(() => useLibraryStore.getState().resetLibraryDetailWidth());
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

  it('shows a loading state when fetching with no index yet', () => {
    useLibraryStore.setState({ index: null, isLoading: true, error: null });

    render(<LibraryView selectedAgentIds={new Set()} />);

    expect(screen.getByTestId('library-loading')).toBeInTheDocument();
    expect(screen.queryByTestId('library-section-rail')).not.toBeInTheDocument();
  });

  it('shows an error state with retry when the initial fetch fails', () => {
    const fetchIndex = vi.fn();
    useLibraryStore.setState({ index: null, isLoading: false, error: 'boom', fetchIndex });

    render(<LibraryView selectedAgentIds={new Set()} />);

    expect(screen.getByTestId('library-error')).toHaveTextContent('boom');
    expect(screen.queryByTestId('library-section-rail')).not.toBeInTheDocument();

    fireEvent.click(screen.getByTestId('library-retry'));

    expect(fetchIndex).toHaveBeenCalledTimes(1);
  });

  it('keeps the loaded content visible and shows a banner when a background refetch fails', () => {
    const fetchIndex = vi.fn();
    useLibraryStore.setState({ index: emptyIndex, isLoading: false, error: 'refresh failed', fetchIndex });

    render(<LibraryView selectedAgentIds={new Set()} />);

    expect(screen.getByTestId('library-error-banner')).toHaveTextContent('refresh failed');
    expect(screen.getByTestId('library-section-rail')).toBeInTheDocument();
    expect(screen.getByTestId('library-list')).toBeInTheDocument();
    expect(screen.getByTestId('library-detail')).toBeInTheDocument();

    fireEvent.click(screen.getByTestId('library-error-banner-retry'));

    expect(fetchIndex).toHaveBeenCalledTimes(1);
  });

  it('renders a resize handle between the list and detail panes', () => {
    render(<LibraryView selectedAgentIds={new Set()} />);

    const detail = screen.getByTestId('library-detail');
    const handle = screen.getByTestId('sidebar-resize-handle');

    expect(handle).toBeInTheDocument();
    expect(detail).toContainElement(handle);
    expect(detail).toHaveStyle({ width: `${useLibraryStore.getState().libraryDetailWidth}px` });
  });

  it('dragging the resize handle widens the detail pane and persists the width', () => {
    render(<LibraryView selectedAgentIds={new Set()} />);

    const startWidth = useLibraryStore.getState().libraryDetailWidth;
    const handle = screen.getByTestId('sidebar-resize-handle');

    // edge="left": dragging left (negative delta) grows the right-anchored detail pane.
    fireEvent.pointerDown(handle, { clientX: 500 });
    fireEvent.pointerMove(window, { clientX: 440 });
    fireEvent.pointerUp(window, { clientX: 440 });

    const nextWidth = useLibraryStore.getState().libraryDetailWidth;
    expect(nextWidth).toBe(startWidth + 60);
    expect(screen.getByTestId('library-detail')).toHaveStyle({ width: `${nextWidth}px` });
  });

  it('double-clicking the resize handle resets the detail pane to the default width', () => {
    render(<LibraryView selectedAgentIds={new Set()} />);

    const handle = screen.getByTestId('sidebar-resize-handle');

    fireEvent.pointerDown(handle, { clientX: 500 });
    fireEvent.pointerMove(window, { clientX: 300 });
    fireEvent.pointerUp(window, { clientX: 300 });

    expect(useLibraryStore.getState().libraryDetailWidth).not.toBe(480);

    fireEvent.doubleClick(handle);

    expect(useLibraryStore.getState().libraryDetailWidth).toBe(480);
  });

  it('never shrinks the list pane below its minimum width', () => {
    render(<LibraryView selectedAgentIds={new Set()} />);

    expect(screen.getByTestId('library-list')).toHaveClass('min-w-[320px]');
  });
});
