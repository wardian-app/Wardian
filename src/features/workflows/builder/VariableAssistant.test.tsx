import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { VariableAssistant } from './VariableAssistant';
import type { Blueprint } from './blueprintTypes';

const blueprint: Blueprint = {
  schema: 2,
  id: 'wf',
  name: 'Workflow',
  nodes: [
    { id: 'trigger-1', type: 'manual_trigger', fields: {} },
    { id: 'agent-1', type: 'task', fields: { agent: 'role:x', prompt: 'p' } },
    { id: 'notify-action', type: 'notify', fields: { message: 'hello' } },
  ],
  edges: [
    { from: 'trigger-1', to: 'agent-1', from_port: 'out', to_port: 'in' },
    { from: 'agent-1', to: 'notify-action', from_port: 'out', to_port: 'in' },
  ],
};

describe('VariableAssistant', () => {
  it('renders as a compact insert-variable helper with truncated token rows', () => {
    render(<VariableAssistant blueprint={blueprint} selectedNodeId="notify-action" />);

    expect(screen.getByText('Insert variable')).toBeVisible();
    expect(screen.queryByText('Variable Assistant')).toBeNull();
    expect(screen.getByTitle('{{nodes.agent-1.output}}')).toHaveClass('h-8');
    expect(screen.getByTitle('{{nodes.agent-1.output}}').querySelector('span')).toHaveClass('truncate');
  });

  it('copies variables from compact rows', () => {
    const writeText = vi.fn();
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    });
    render(<VariableAssistant blueprint={blueprint} selectedNodeId="notify-action" />);

    fireEvent.click(screen.getByTitle('{{nodes.agent-1.output}}'));

    expect(writeText).toHaveBeenCalledWith('{{nodes.agent-1.output}}');
  });
});
