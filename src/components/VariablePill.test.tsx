import { render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { useWorkflowStore } from '../store/useWorkflowStore';
import { VariablePill } from './VariablePill';

describe('VariablePill', () => {
  beforeEach(() => {
    useWorkflowStore.setState({ nodes: [], edges: [] });
  });

  it('renders node variables with the node label and nested path', () => {
    useWorkflowStore.setState({
      nodes: [
        {
          id: 'cmd',
          type: 'command',
          data: { label: 'Build Step' },
        } as any,
      ],
    });

    render(<VariablePill path="{{nodes.cmd.output.stdout}}" isPrevious />);

    expect(screen.getByTitle('{{nodes.cmd.output.stdout}}')).toBeInTheDocument();
    expect(screen.getByText('Build Step')).toBeInTheDocument();
    expect(screen.getByText('output.stdout')).toBeInTheDocument();
    expect(screen.getByText('PREV')).toBeInTheDocument();
  });

  it('renders trigger, storage, and fallback variables', () => {
    const { rerender } = render(<VariablePill path="trigger.payload.id" />);
    expect(screen.getByText('Trigger')).toBeInTheDocument();
    expect(screen.getByText('payload.id')).toBeInTheDocument();

    rerender(<VariablePill path="storage.release.version" />);
    expect(screen.getByText('Storage')).toBeInTheDocument();
    expect(screen.getByText('release.version')).toBeInTheDocument();

    rerender(<VariablePill path="customValue" />);
    expect(screen.getByText('customValue')).toBeInTheDocument();
  });
});
