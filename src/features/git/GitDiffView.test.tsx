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

  it('renders review actions for the opened diff', async () => {
    const user = userEvent.setup();
    const onStage = vi.fn();
    const onUnstage = vi.fn();

    render(
      <GitDiffView
        filePath="src/app.tsx"
        diff="+new"
        onClose={() => {}}
        actions={[
          { label: 'Stage Changes', onClick: onStage },
          { label: 'Unstage Changes', onClick: onUnstage },
        ]}
      />,
    );

    await user.click(screen.getByRole('button', { name: 'Stage Changes' }));
    await user.click(screen.getByRole('button', { name: 'Unstage Changes' }));

    expect(onStage).toHaveBeenCalledTimes(1);
    expect(onUnstage).toHaveBeenCalledTimes(1);
  });

  it('renders hunk review actions with a single-hunk patch', async () => {
    const user = userEvent.setup();
    const onStageHunk = vi.fn();

    render(
      <GitDiffView
        filePath="src/app.tsx"
        onClose={() => {}}
        diff={[
          'diff --git a/src/app.tsx b/src/app.tsx',
          'index 1111111..2222222 100644',
          '--- a/src/app.tsx',
          '+++ b/src/app.tsx',
          '@@ -1,3 +1,3 @@',
          ' one',
          '-two',
          '+TWO',
          ' three',
          '@@ -10,3 +10,3 @@',
          ' ten',
          '-eleven',
          '+ELEVEN',
          ' twelve',
        ].join('\n')}
        hunkActions={[{ label: 'Stage Hunk', onClick: onStageHunk }]}
      />,
    );

    const stageHunkButtons = screen.getAllByRole('button', { name: 'Stage Hunk' });
    expect(stageHunkButtons).toHaveLength(2);

    await user.click(stageHunkButtons[0]);

    expect(onStageHunk).toHaveBeenCalledTimes(1);
    expect(onStageHunk).toHaveBeenCalledWith(
      [
        'diff --git a/src/app.tsx b/src/app.tsx',
        'index 1111111..2222222 100644',
        '--- a/src/app.tsx',
        '+++ b/src/app.tsx',
        '@@ -1,3 +1,3 @@',
        ' one',
        '-two',
        '+TWO',
        ' three',
        '',
      ].join('\n'),
    );
  });
});
