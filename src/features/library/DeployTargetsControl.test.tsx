import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { DeployTargetsControl } from './DeployTargetsControl';
import { useLibraryStore } from '../../store/useLibraryStore';
import { DeploymentTarget, LibraryEntry, LibraryIndex } from '../../types';

const mockInvoke = vi.mocked(invoke);

function skillEntry(overrides: Partial<LibraryEntry> = {}): LibraryEntry {
  return {
    kind: 'skill',
    path: 'dev/planner',
    entry_ref: 'skills/dev/planner',
    name: 'planner',
    description: '',
    tags: [],
    is_starred: false,
    deployment_count: 0,
    error: null,
    ...overrides,
  };
}

function classesIndex(): LibraryIndex {
  const emptyTree = { path: '', name: 'Root', children: [] };
  return {
    sections: {
      skills: { stubbed: false, tree: emptyTree },
      prompts: { stubbed: false, tree: emptyTree },
      workflows: { stubbed: false, tree: emptyTree },
      classes: {
        stubbed: false,
        tree: {
          path: '',
          name: 'Root',
          children: [
            {
              kind: 'class',
              path: 'Architect',
              entry_ref: 'classes/Architect',
              name: 'Architect',
              description: '',
              tags: [],
              is_starred: false,
              deployment_count: 0,
              error: null,
            },
            {
              kind: 'class',
              path: 'Coder',
              entry_ref: 'classes/Coder',
              name: 'Coder',
              description: '',
              tags: [],
              is_starred: false,
              deployment_count: 0,
              error: null,
            },
          ],
        },
      },
      mcps: { stubbed: true, tree: emptyTree },
    },
    deployments: {},
    orphans: [],
  };
}

describe('DeployTargetsControl', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'list_agents') {
        return [
          { session_id: 'agent-1', session_name: 'Coder One', agent_class: 'Coder', folder: '/w', is_off: false, provider: 'mock' },
        ];
      }
      return undefined;
    });
    useLibraryStore.setState({ index: classesIndex(), select: vi.fn().mockResolvedValue(undefined) });
  });

  it('renders User (global), classes from the index, and persisted agents', async () => {
    render(<DeployTargetsControl entry={skillEntry()} deployments={[]} onApply={vi.fn()} />);

    expect(screen.getByText('User (global)')).toBeInTheDocument();
    expect(screen.getByText('Architect')).toBeInTheDocument();
    expect(screen.getByText('Coder')).toBeInTheDocument();
    expect(await screen.findByText('Coder One')).toBeInTheDocument();
  });

  it('checks boxes that match existing deployments', async () => {
    const deployments: DeploymentTarget[] = [{ target_type: 'class', target_id: 'Architect', linked: true }];
    render(<DeployTargetsControl entry={skillEntry()} deployments={deployments} onApply={vi.fn()} />);

    await waitFor(() => expect(screen.getByTestId('deploy-target-class:Architect')).toBeInTheDocument());
    const checkbox = screen.getByTestId('deploy-target-class:Architect').querySelector('input[type="checkbox"]');
    expect(checkbox).toBeChecked();
    const otherCheckbox = screen.getByTestId('deploy-target-user:global').querySelector('input[type="checkbox"]');
    expect(otherCheckbox).not.toBeChecked();
  });

  it('checking/unchecking targets and applying calls onApply with the full desired set', async () => {
    const onApply = vi.fn();
    const deployments: DeploymentTarget[] = [{ target_type: 'class', target_id: 'Architect', linked: true }];
    render(<DeployTargetsControl entry={skillEntry()} deployments={deployments} onApply={onApply} />);

    await screen.findByText('Coder One');

    // Uncheck the existing Architect deployment, check User (global).
    fireEvent.click(screen.getByTestId('deploy-target-class:Architect').querySelector('input[type="checkbox"]')!);
    fireEvent.click(screen.getByTestId('deploy-target-user:global').querySelector('input[type="checkbox"]')!);
    fireEvent.click(screen.getByTestId('deploy-targets-apply'));

    expect(onApply).toHaveBeenCalledWith([{ target_type: 'user', target_id: 'global' }]);
  });

  it('shows the "copied" note for unlinked deployments', async () => {
    const deployments: DeploymentTarget[] = [{ target_type: 'agent', target_id: 'agent-1', linked: false }];
    render(<DeployTargetsControl entry={skillEntry()} deployments={deployments} onApply={vi.fn()} />);

    await screen.findByText('Coder One');
    expect(screen.getByTestId('deploy-target-copied-agent:agent-1')).toHaveTextContent("copied — edits won't sync");
  });

  it('does not show the "copied" note for linked deployments', async () => {
    const deployments: DeploymentTarget[] = [{ target_type: 'class', target_id: 'Architect', linked: true }];
    render(<DeployTargetsControl entry={skillEntry()} deployments={deployments} onApply={vi.fn()} />);

    await waitFor(() => expect(screen.getByTestId('deploy-target-class:Architect')).toBeInTheDocument());
    expect(screen.queryByTestId('deploy-target-copied-class:Architect')).not.toBeInTheDocument();
  });

  it('accepts a drop of a different skill ref and switches selection to it', async () => {
    const select = vi.fn().mockResolvedValue(undefined);
    useLibraryStore.setState({ select });
    render(<DeployTargetsControl entry={skillEntry()} deployments={[]} onApply={vi.fn()} />);

    const dataTransfer = { getData: () => 'skills/other/tool' } as unknown as DataTransfer;
    fireEvent.drop(screen.getByTestId('deploy-targets-control'), { dataTransfer });

    expect(select).toHaveBeenCalledWith('skills/other/tool');
  });
});
