import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { SchemaEditor } from './SchemaEditor';

vi.mock('./RenderableInput', () => ({
  RenderableInput: ({ value, onChange }: { value: string; onChange: (value: string) => void }) => (
    <textarea
      aria-label="JSON schema source"
      value={value}
      onChange={(event) => onChange(event.currentTarget.value)}
    />
  ),
}));

const lastSchemaChange = (onChange: ReturnType<typeof vi.fn>) =>
  JSON.parse(onChange.mock.calls[onChange.mock.calls.length - 1]?.[0] as string);

describe('SchemaEditor', () => {
  it('adds, renames, and removes top-level properties', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<SchemaEditor value='{"type":"object","properties":{}}' onChange={onChange} nodeId="node-1" />);

    expect(screen.getByText('No Fields Defined')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: /Add Property/i }));

    expect(lastSchemaChange(onChange).properties).toHaveProperty('field_1');
    const nameInput = screen.getByDisplayValue('field_1');
    await user.clear(nameInput);
    await user.type(nameInput, 'summary');
    fireEvent.blur(nameInput);

    expect(lastSchemaChange(onChange).properties).toHaveProperty('summary');
    expect(lastSchemaChange(onChange).properties).not.toHaveProperty('field_1');

    await user.click(screen.getByRole('button', { name: 'Remove summary' }));
    expect(lastSchemaChange(onChange).properties).not.toHaveProperty('summary');
  });

  it('creates nested object fields and avoids rename collisions', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <SchemaEditor
        value={JSON.stringify({
          type: 'object',
          properties: {
            existing: { type: 'string' },
            nested: { type: 'object', properties: {} },
          },
        })}
        onChange={onChange}
        nodeId="node-1"
      />,
    );

    await user.click(screen.getByRole('button', { name: /Add Subfield/i }));
    expect(lastSchemaChange(onChange).properties.nested.properties).toHaveProperty('field_1');

    const nestedInput = screen.getByDisplayValue('nested');
    await user.clear(nestedInput);
    await user.type(nestedInput, 'existing');
    fireEvent.blur(nestedInput);

    const schema = lastSchemaChange(onChange);
    expect(schema.properties).toHaveProperty('existing');
    expect(schema.properties).toHaveProperty('existing_1');
    expect(schema.properties).not.toHaveProperty('nested');
  });

  it('switches to JSON editing and forwards source changes', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<SchemaEditor value='{"type":"object","properties":{}}' onChange={onChange} nodeId="node-1" />);

    await user.click(screen.getByRole('button', { name: 'JSON' }));
    await user.clear(screen.getByLabelText('JSON schema source'));
    fireEvent.change(screen.getByLabelText('JSON schema source'), {
      target: { value: '{"type":"object"}' },
    });

    expect(onChange).toHaveBeenLastCalledWith('{"type":"object"}');
  });
});
