import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import { RunDag } from './RunDag';
import type { Blueprint } from '../builder/blueprintTypes';

const fitViewMock = vi.fn();

vi.mock('@xyflow/react', () => ({
  ReactFlowProvider: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  ReactFlow: ({ nodes, nodeTypes }: { nodes: any[]; nodeTypes: Record<string, React.ComponentType<any>> }) => (
    <div data-testid="mock-flow">
      {nodes.map((node) => {
        const NodeComponent = nodeTypes[node.type];
        return <NodeComponent key={node.id} data={node.data} selected={node.selected} />;
      })}
    </div>
  ),
  Background: () => null,
  Controls: () => null,
  MiniMap: () => null,
  Handle: ({ id }: { id?: string }) => <span data-testid={`handle-${id ?? 'default'}`} />,
  useReactFlow: () => ({ fitView: fitViewMock }),
  BackgroundVariant: { Dots: 'dots' },
  Position: { Left: 'left', Right: 'right' },
}));

const blueprint: Blueprint = {
  schema: 2,
  id: 'wf',
  name: 'Workflow',
  nodes: [
    {
      id: 'task-1',
      type: 'task',
      name: 'Review',
      fields: { agent: 'role:reviewer', prompt: 'Review the latest diff for correctness issues.' },
    },
  ],
  edges: [],
};

describe('RunDag', () => {
  beforeEach(() => {
    fitViewMock.mockClear();
  });

  it('renders human-readable node status directly on the graph', () => {
    render(
      <RunDag
        blueprint={blueprint}
        currentStatuses={{ 'task-1': 'failed' }}
        selectedNodeId={null}
        onSelectNode={() => {}}
        theme="dark"
      />,
    );

    const task = screen.getByTestId('run-dag-node-task-1');
    expect(task).toHaveAttribute('data-status', 'failed');
    expect(within(task).getByText('Failed')).toBeVisible();
  });

  it('refits the viewport when a run blueprint loads after an empty observe surface', async () => {
    const { rerender } = render(
      <RunDag
        blueprint={null}
        currentStatuses={{}}
        selectedNodeId={null}
        onSelectNode={() => {}}
        theme="dark"
      />,
    );

    expect(fitViewMock).not.toHaveBeenCalled();

    rerender(
      <RunDag
        blueprint={blueprint}
        currentStatuses={{ 'task-1': 'running' }}
        selectedNodeId={null}
        onSelectNode={() => {}}
        theme="dark"
      />,
    );

    await waitFor(() => expect(fitViewMock).toHaveBeenCalledWith({ padding: 0.2, duration: 120 }));
  });
});
