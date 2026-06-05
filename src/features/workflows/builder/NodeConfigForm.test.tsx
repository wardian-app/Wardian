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
  it('uses vertical, full-width field blocks with useful textarea room', () => {
    render(<NodeConfigForm node={taskNode} onChange={() => {}} />);

    const promptField = screen.getByTestId('field-prompt');
    const prompt = screen.getByLabelText(/Prompt/i);

    expect(promptField).toHaveClass('grid');
    expect(prompt).toHaveClass('min-h-[132px]');
    expect(prompt).toHaveClass('w-full');
  });
  it('shows templated loop max iterations as editable text', () => {
    const onChange = vi.fn();
    const loopNode: BlueprintNode = {
      id: 'loop-1',
      type: 'loop',
      fields: {
        max_iterations: '{{trigger.output.max_cycles}}',
        until: 'nodes.agent-arbiter.output.converged',
      },
    };

    render(<NodeConfigForm node={loopNode} onChange={onChange} />);

    const maxIterations = screen.getByLabelText(/Max iterations/i) as HTMLInputElement;
    expect(maxIterations).toHaveAttribute('type', 'text');
    expect(maxIterations).toHaveValue('{{trigger.output.max_cycles}}');

    fireEvent.change(maxIterations, { target: { value: '7' } });

    expect(onChange).toHaveBeenCalledWith('max_iterations', 7);

    fireEvent.change(maxIterations, { target: { value: '{{trigger.output.review_cap}}' } });

    expect(onChange).toHaveBeenLastCalledWith('max_iterations', '{{trigger.output.review_cap}}');
  });
});
