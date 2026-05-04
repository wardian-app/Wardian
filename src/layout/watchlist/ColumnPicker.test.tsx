import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ColumnPicker } from './ColumnPicker';
import { DEFAULT_WATCHLIST_PREFS } from './types';

describe('ColumnPicker', () => {
  it('toggles optional columns without closing the picker', async () => {
    const user = userEvent.setup();
    const onPrefsChange = vi.fn();
    const onClose = vi.fn();

    render(
      <ColumnPicker
        prefs={DEFAULT_WATCHLIST_PREFS}
        onPrefsChange={onPrefsChange}
        onClose={onClose}
      />,
    );

    await user.click(screen.getByRole('checkbox', { name: 'Status' }));

    expect(onPrefsChange).toHaveBeenCalledWith({
      ...DEFAULT_WATCHLIST_PREFS,
      columns: DEFAULT_WATCHLIST_PREFS.columns.map((column) =>
        column.id === 'status_label' ? { ...column, visible: false } : column,
      ),
    });
    expect(onClose).not.toHaveBeenCalled();
  });

  it('toggles sorted team grouping and closes on outside click', async () => {
    const user = userEvent.setup();
    const onPrefsChange = vi.fn();
    const onClose = vi.fn();

    render(
      <ColumnPicker
        prefs={DEFAULT_WATCHLIST_PREFS}
        onPrefsChange={onPrefsChange}
        onClose={onClose}
      />,
    );

    await user.click(screen.getByRole('checkbox', { name: 'Preserve Teams While Sorted' }));
    expect(onPrefsChange).toHaveBeenCalledWith({
      ...DEFAULT_WATCHLIST_PREFS,
      preserve_team_grouping_when_sorted: true,
    });

    await user.click(document.body);
    expect(onClose).toHaveBeenCalled();
  });
});
