import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { LibraryPrompt } from '../../types';
import { ItemEditorModal } from './ItemEditorModal';

const prompt: LibraryPrompt = {
  type: 'Prompt',
  path: 'prompts/review.md',
  name: 'Review Prompt',
  content: 'Original content',
  metadata: {
    id: 'prompt-1',
    tags: ['review', 'code'],
    is_starred: false,
    last_used: '2026-05-04T12:00:00Z',
  },
};

describe('ItemEditorModal', () => {
  it('does not render while closed', () => {
    render(<ItemEditorModal item={prompt} isOpen={false} onClose={() => {}} onSave={() => {}} />);

    expect(screen.queryByText('Review Prompt')).not.toBeInTheDocument();
  });

  it('saves edited content, parsed tags, and starred state', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    const onSave = vi.fn();
    render(<ItemEditorModal item={prompt} isOpen onClose={onClose} onSave={onSave} />);

    fireEvent.change(screen.getByLabelText('Content'), { target: { value: 'Updated content' } });
    fireEvent.change(screen.getByLabelText('Tags (comma separated)'), {
      target: { value: 'review, coverage, , scheduler ' },
    });
    await user.click(screen.getByRole('button', { name: 'Star item' }));
    await user.click(screen.getByRole('button', { name: 'Save Changes' }));

    expect(onSave).toHaveBeenCalledWith('prompts/review.md', 'Updated content', {
      id: 'prompt-1',
      tags: ['review', 'coverage', 'scheduler'],
      is_starred: true,
      last_used: '2026-05-04T12:00:00Z',
    });
    expect(onClose).toHaveBeenCalled();
  });

  it('resets local edits when reopened', async () => {
    const user = userEvent.setup();
    const { rerender } = render(
      <ItemEditorModal item={prompt} isOpen onClose={() => {}} onSave={() => {}} />,
    );

    fireEvent.change(screen.getByLabelText('Content'), { target: { value: 'Unsaved edit' } });
    fireEvent.change(screen.getByLabelText('Tags (comma separated)'), {
      target: { value: 'unsaved' },
    });
    await user.click(screen.getByRole('button', { name: 'Star item' }));
    expect(screen.getByRole('button', { name: 'Unstar item' })).toBeInTheDocument();

    rerender(<ItemEditorModal item={prompt} isOpen={false} onClose={() => {}} onSave={() => {}} />);
    expect(screen.queryByText('Review Prompt')).not.toBeInTheDocument();

    rerender(<ItemEditorModal item={prompt} isOpen onClose={() => {}} onSave={() => {}} />);

    expect(screen.getByLabelText('Content')).toHaveValue('Original content');
    expect(screen.getByLabelText('Tags (comma separated)')).toHaveValue('review, code');
    expect(screen.getByRole('button', { name: 'Star item' })).toBeInTheDocument();
  });
});
