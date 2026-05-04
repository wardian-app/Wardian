import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { GitDiffView } from './GitDiffView';

describe('GitDiffView', () => {
  it('renders highlighted diff lines and closes from the backdrop or button', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();

    render(
      <GitDiffView
        filePath="src/app.tsx"
        onClose={onClose}
        diff={['diff --git a/src/app.tsx b/src/app.tsx', '@@ -1 +1 @@', '-old', '+new'].join('\n')}
      />,
    );

    expect(screen.getByText('src/app.tsx')).toBeInTheDocument();
    expect(screen.getByText('@@ -1 +1 @@')).toHaveStyle({
      color: 'var(--color-wardian-processing)',
    });
    expect(screen.getByText('-old')).toHaveStyle({ color: 'var(--color-wardian-error)' });
    expect(screen.getByText('+new')).toHaveStyle({ color: 'var(--color-wardian-success)' });

    await user.click(screen.getByRole('button', { name: 'Close diff' }));
    expect(onClose).toHaveBeenCalledTimes(1);

    await user.click(screen.getByText('src/app.tsx'));
    expect(onClose).toHaveBeenCalledTimes(1);

    await user.click(screen.getByTestId('git-diff-backdrop'));
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it('renders an empty state when there is no diff', () => {
    render(<GitDiffView filePath="README.md" diff="" onClose={() => {}} />);

    expect(screen.getByText('No differences to display.')).toBeInTheDocument();
  });
});
