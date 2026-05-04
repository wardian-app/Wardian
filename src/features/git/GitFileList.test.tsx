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
});
