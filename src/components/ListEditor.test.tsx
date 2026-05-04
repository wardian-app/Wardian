import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ConfirmProvider } from './ConfirmDialog';
import { ListEditor } from './ListEditor';

describe('ListEditor', () => {
  it('adds, edits, and removes user values', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const { rerender } = render(
      <ListEditor label="Paths" values={['C:/project']} onChange={onChange} />,
    );

    await user.click(screen.getByRole('button', { name: /Add paths/i }));
    expect(onChange).toHaveBeenLastCalledWith(['C:/project', '']);

    rerender(<ListEditor label="Paths" values={['C:/project', '']} onChange={onChange} />);
    fireEvent.change(screen.getAllByRole('textbox')[1], { target: { value: 'D:/workspace' } });
    expect(onChange).toHaveBeenLastCalledWith(['C:/project', 'D:/workspace']);

    rerender(<ListEditor label="Paths" values={['C:/project', 'D:/workspace']} onChange={onChange} />);
    await user.click(screen.getByRole('button', { name: 'Remove paths value 1' }));
    expect(onChange).toHaveBeenLastCalledWith(['D:/workspace']);
  });

  it('validates values and keeps read-only system values from editing directly', async () => {
    const validate = vi.fn(async (value: string) => value.startsWith('C:/'));
    const onChange = vi.fn();

    render(
      <ListEditor
        label="Include Directories"
        values={['D:/invalid']}
        systemValues={['C:/system']}
        onChange={onChange}
        validate={validate}
      />,
    );

    await waitFor(() => {
      expect(screen.getByTitle('Valid path')).toBeInTheDocument();
      expect(screen.getByTitle('Invalid or missing path')).toBeInTheDocument();
    });

    const systemInput = screen.getByDisplayValue('C:/system');
    await userEvent.type(systemInput, 'ignored');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('confirms before deleting a system value', async () => {
    const user = userEvent.setup();
    const onSystemValueDelete = vi.fn();

    render(
      <ConfirmProvider>
        <ListEditor
          label="Include Directories"
          values={[]}
          systemValues={['C:/system']}
          onChange={() => {}}
          onSystemValueDelete={onSystemValueDelete}
        />
      </ConfirmProvider>,
    );

    await user.click(screen.getByTitle('Remove system directory'));
    expect(screen.getByText(/system-managed directory/i)).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Confirm' }));
    expect(onSystemValueDelete).toHaveBeenCalledWith(0);
  });
});
