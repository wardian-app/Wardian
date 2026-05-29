import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { NodeConfigForm } from './NodeConfigForm';
import type { BlueprintNode } from './blueprintTypes';

const taskNode: BlueprintNode = { id: 'plan', type: 'task', fields: { agent: 'role:x', prompt: 'hi' } };

describe('NodeConfigForm', () => {
  it('renders an input per registry field (prompt as textarea, agent as ref)', () => {
    render(<NodeConfigForm node={taskNode} onChange={() => {}} />);
    expect(screen.getByLabelText(/Prompt/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/Agent/i)).toBeInTheDocument();
  });
  it('emits field changes', () => {
    const onChange = vi.fn();
    render(<NodeConfigForm node={taskNode} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText(/Prompt/i), { target: { value: 'new' } });
    expect(onChange).toHaveBeenCalledWith('prompt', 'new');
  });
  it('renders enum fields as a select with options', () => {
    const scriptNode: BlueprintNode = { id: 's', type: 'script', fields: { runtime: 'python', path: 'x.py' } };
    render(<NodeConfigForm node={scriptNode} onChange={() => {}} />);
    const select = screen.getByLabelText(/Runtime/i) as HTMLSelectElement;
    expect([...select.options].map((o) => o.value)).toEqual(['python', 'node', 'sh']);
  });
});
