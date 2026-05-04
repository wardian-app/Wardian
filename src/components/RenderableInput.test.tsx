import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { RenderableInput } from './RenderableInput';

describe('RenderableInput', () => {
  it('shows a placeholder when empty', () => {
    render(<RenderableInput value="" onChange={() => {}} placeholder="Describe input" />);

    expect(screen.getByText('Describe input')).toBeInTheDocument();
  });

  it('renders variable tags and switches to an editable input on click', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <RenderableInput
        value="Run {{trigger.payload.id}}"
        onChange={onChange}
        placeholder="Describe input"
      />,
    );

    expect(screen.getByText('Run')).toBeInTheDocument();
    expect(screen.getByText('Trigger')).toBeInTheDocument();
    expect(screen.getByText('payload.id')).toBeInTheDocument();

    await user.click(screen.getByText('Run'));
    const input = screen.getByRole('textbox');
    expect(input).toHaveValue('Run {{trigger.payload.id}}');

    fireEvent.change(input, { target: { value: 'Run {{storage.release.version}}' } });
    expect(onChange).toHaveBeenCalledWith('Run {{storage.release.version}}');
  });

  it('uses a textarea for multiline editing', async () => {
    const user = userEvent.setup();
    render(<RenderableInput value="line one" onChange={() => {}} multiline />);

    await user.click(screen.getByText('line one'));

    expect(screen.getByRole('textbox').tagName).toBe('TEXTAREA');
  });
});
