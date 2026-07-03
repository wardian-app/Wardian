import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { GitFileEntry } from '../../types';
import { GitFileList } from './GitFileList';

const files: GitFileEntry[] = [
  { path: 'src/app.tsx', status: 'M', is_staged: false },
  { path: 'README.md', status: '?', is_staged: false },
  { path: 'src/main.ts', status: 'A', is_staged: true },
];

describe('GitFileList', () => {
  it('renders nothing for empty file lists', () => {
    const { container } = render(<GitFileList files={[]} />);

    expect(container).toBeEmptyDOMElement();
  });

  it('routes file and action clicks to the right callbacks', async () => {
    const user = userEvent.setup();
    const onDiff = vi.fn();
    const onStage = vi.fn();
    const onUnstage = vi.fn();
    const onDiscard = vi.fn();

    render(
      <GitFileList
        files={files}
        onDiff={onDiff}
        onStage={onStage}
        onUnstage={onUnstage}
        onDiscard={onDiscard}
      />,
    );

    await user.click(screen.getByRole('button', { name: 'View diff for src/app.tsx' }));
    expect(onDiff).toHaveBeenCalledWith('src/app.tsx', false);

    await user.click(screen.getByRole('button', { name: 'Stage src/app.tsx' }));
    expect(onStage).toHaveBeenCalledWith('src/app.tsx');

    await user.click(screen.getByRole('button', { name: 'Discard changes to src/app.tsx' }));
    expect(onDiscard).toHaveBeenCalledWith('src/app.tsx');

    await user.click(screen.getByRole('button', { name: 'Unstage src/main.ts' }));
    expect(onUnstage).toHaveBeenCalledWith('src/main.ts');
  });

  it('labels status badges and strikes deleted file names like VS Code resource rows', () => {
    render(
      <GitFileList
        files={[
          { path: 'src/removed.ts', status: 'D', is_staged: false },
          { path: 'src/conflicted.ts', status: 'UU', is_staged: false },
        ]}
      />,
    );

    expect(screen.getByLabelText('Deleted')).toHaveTextContent('D');
    expect(screen.getByText('removed.ts')).toHaveClass('line-through');
    expect(screen.getByLabelText('Both Modified')).toHaveTextContent('UU');
  });

  it('opens file context actions on right click', async () => {
    const user = userEvent.setup();
    const onDiff = vi.fn();
    const onStage = vi.fn();
    const onDiscard = vi.fn();

    render(
      <GitFileList
        files={[{ path: 'src/app.tsx', status: 'M', is_staged: false }]}
        onDiff={onDiff}
        onStage={onStage}
        onDiscard={onDiscard}
      />,
    );

    await user.pointer({ keys: '[MouseRight]', target: screen.getByRole('button', { name: 'View diff for src/app.tsx' }) });
    await user.click(await screen.findByRole('button', { name: 'Stage' }));

    expect(onStage).toHaveBeenCalledWith('src/app.tsx');
    expect(onDiff).not.toHaveBeenCalled();
  });

  it('renders nested paths as expandable tree rows', async () => {
    const user = userEvent.setup();
    const onDiff = vi.fn();

    render(
      <GitFileList
        displayMode="tree"
        files={[
          { path: 'src/components/App.tsx', status: 'M', is_staged: false },
          { path: 'src/main.ts', status: 'A', is_staged: false },
          { path: 'README.md', status: '?', is_staged: false },
        ]}
        onDiff={onDiff}
      />,
    );

    const srcFolder = screen.getByRole('button', { name: 'src' });
    expect(srcFolder).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByRole('button', { name: 'components' })).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByRole('button', { name: 'View diff for src/components/App.tsx' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'View diff for src/main.ts' })).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'components' }));

    expect(screen.queryByRole('button', { name: 'View diff for src/components/App.tsx' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'View diff for src/main.ts' })).toBeInTheDocument();
  });

  it('orders resources by VS Code-style status priority before path while preserving tree hierarchy', () => {
    const priorityFiles: GitFileEntry[] = [
      { path: 'zeta/new-file.ts', status: '?', is_staged: false },
      { path: 'alpha/added.ts', status: 'A', is_staged: false },
      { path: 'zeta/modified.ts', status: 'M', is_staged: false },
      { path: 'alpha/conflict.ts', status: 'UU', is_staged: false },
      { path: 'beta/modified.ts', status: 'M', is_staged: false },
    ];

    const { rerender } = render(<GitFileList files={priorityFiles} displayMode="list" />);

    expect(screen.getAllByRole('button', { name: /View diff for/ }).map((button) => button.getAttribute('aria-label'))).toEqual([
      'View diff for alpha/conflict.ts',
      'View diff for beta/modified.ts',
      'View diff for zeta/modified.ts',
      'View diff for alpha/added.ts',
      'View diff for zeta/new-file.ts',
    ]);

    rerender(<GitFileList files={priorityFiles} displayMode="tree" />);

    expect(screen.getAllByRole('button', { name: /View diff for/ }).map((button) => button.getAttribute('aria-label'))).toEqual([
      'View diff for alpha/conflict.ts',
      'View diff for alpha/added.ts',
      'View diff for beta/modified.ts',
      'View diff for zeta/modified.ts',
      'View diff for zeta/new-file.ts',
    ]);
  });

  it('supports VS Code-style resource sort modes for list rows', () => {
    const sortableFiles: GitFileEntry[] = [
      { path: 'beta/zeta.ts', status: '?', is_staged: false },
      { path: 'alpha/readme.md', status: 'M', is_staged: false },
      { path: 'gamma/app.ts', status: 'A', is_staged: false },
    ];

    const labels = () =>
      screen.getAllByRole('button', { name: /View diff for/ }).map((button) => button.getAttribute('aria-label'));

    const { rerender } = render(<GitFileList files={sortableFiles} displayMode="list" sortMode="path" />);

    expect(labels()).toEqual([
      'View diff for alpha/readme.md',
      'View diff for beta/zeta.ts',
      'View diff for gamma/app.ts',
    ]);

    rerender(<GitFileList files={sortableFiles} displayMode="list" sortMode="name" />);

    expect(labels()).toEqual([
      'View diff for gamma/app.ts',
      'View diff for alpha/readme.md',
      'View diff for beta/zeta.ts',
    ]);

    rerender(<GitFileList files={sortableFiles} displayMode="list" sortMode="status" />);

    expect(labels()).toEqual([
      'View diff for alpha/readme.md',
      'View diff for beta/zeta.ts',
      'View diff for gamma/app.ts',
    ]);
  });
});
