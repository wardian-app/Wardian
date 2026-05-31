import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import { BuilderCanvas } from './BuilderCanvas';
import type { Blueprint } from './blueprintTypes';

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
  addEdge: vi.fn(),
  applyEdgeChanges: vi.fn(),
  applyNodeChanges: vi.fn(),
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
    {
      id: 'branch-1',
      type: 'branch',
      fields: {},
    },
  ],
  edges: [],
};

describe('BuilderCanvas', () => {
  beforeEach(() => {
    fitViewMock.mockClear();
  });

  it('renders node field summaries and validation state on cards', () => {
    render(
      <BuilderCanvas
        blueprint={blueprint}
        diagnostics={[{ severity: 'error', code: 'required', message: 'Condition required', node: 'branch-1' }]}
        selectedNodeId={null}
        onSelectNode={() => {}}
        theme="dark"
      />,
    );

    const task = screen.getByTestId('builder-node-task-1');
    expect(within(task).getByText('role:reviewer')).toBeVisible();
    expect(within(task).getByText(/Review the latest diff/i)).toBeVisible();

    const branch = screen.getByTestId('builder-node-branch-1');
    expect(within(branch).getByText('Error')).toBeVisible();
    expect(within(branch).getByText('Condition')).toBeVisible();
    expect(within(branch).getByText('Required')).toBeVisible();
  });

  it('refits the viewport when an existing workflow loads after the empty canvas', async () => {
    const emptyBlueprint: Blueprint = { schema: 2, id: 'empty', name: 'Empty', nodes: [], edges: [] };
    const { rerender } = render(
      <BuilderCanvas
        blueprint={emptyBlueprint}
        diagnostics={[]}
        selectedNodeId={null}
        onSelectNode={() => {}}
        theme="dark"
      />,
    );

    expect(fitViewMock).not.toHaveBeenCalled();

    rerender(
      <BuilderCanvas
        blueprint={blueprint}
        diagnostics={[]}
        selectedNodeId={null}
        onSelectNode={() => {}}
        theme="dark"
      />,
    );

    await waitFor(() => expect(fitViewMock).toHaveBeenCalledWith({ padding: 0.2, duration: 120 }));
  });
});
