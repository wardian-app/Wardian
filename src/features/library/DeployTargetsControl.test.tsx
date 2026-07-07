import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { DeployTargetsControl } from './DeployTargetsControl';
import { useLibraryStore } from '../../store/useLibraryStore';
import { DeploymentTarget, LibraryEntry, LibraryIndex, SkillDeployment } from '../../types';

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

  it('renders a chip per current deployment, with a "copied" marker only on unlinked ones', async () => {
    const deployments: DeploymentTarget[] = [
      { target_type: 'class', target_id: 'Architect', linked: true },
      { target_type: 'agent', target_id: 'agent-1', linked: false },
    ];
    render(<DeployTargetsControl entry={skillEntry()} deployments={deployments} onApply={vi.fn().mockResolvedValue(undefined)} />);

    expect(screen.getByText('Deployed to (2)')).toBeInTheDocument();
    expect(await screen.findByTestId('deploy-chip-class:Architect')).toHaveTextContent('Architect');
    expect(screen.queryByTestId('deploy-chip-copied-class:Architect')).not.toBeInTheDocument();

    const copiedChip = screen.getByTestId('deploy-chip-agent:agent-1');
    expect(copiedChip).toHaveTextContent('Coder One');
    expect(screen.getByTestId('deploy-chip-copied-agent:agent-1')).toHaveAttribute(
      'title',
      "copied — edits won't sync",
    );
  });

  it('renders a chip (with a fallback label) for a deployment with no known target, e.g. a persisted non-live agent', () => {
    const deployments: DeploymentTarget[] = [{ target_type: 'agent', target_id: 'agent-stale', linked: true }];
    render(<DeployTargetsControl entry={skillEntry()} deployments={deployments} onApply={vi.fn().mockResolvedValue(undefined)} />);

    expect(screen.getByTestId('deploy-chip-agent:agent-stale')).toHaveTextContent('agent-stale');
  });

  it('shows an empty-state message when nothing is deployed', () => {
    render(<DeployTargetsControl entry={skillEntry()} deployments={[]} onApply={vi.fn().mockResolvedValue(undefined)} />);
    expect(screen.getByText('Deployed to (0)')).toBeInTheDocument();
    expect(screen.getByText('Not deployed anywhere')).toBeInTheDocument();
  });

  it('removing a chip calls onApply with the full remaining set, preserving every other deployment untouched', async () => {
    const onApply = vi.fn().mockResolvedValue(undefined);
    const deployments: DeploymentTarget[] = [
      { target_type: 'class', target_id: 'Architect', linked: true },
      { target_type: 'agent', target_id: 'agent-stale', linked: true }, // no known label — must still be preserved
      { target_type: 'user', target_id: 'global', linked: true },
    ];
    render(<DeployTargetsControl entry={skillEntry()} deployments={deployments} onApply={onApply} />);

    fireEvent.click(screen.getByTestId('deploy-chip-remove-class:Architect'));

    expect(onApply).toHaveBeenCalledTimes(1);
    const applied = onApply.mock.calls[0][0] as SkillDeployment[];
    expect(applied).toHaveLength(2);
    expect(applied).toEqual(
      expect.arrayContaining([
        { target_type: 'agent', target_id: 'agent-stale' },
        { target_type: 'user', target_id: 'global' },
      ]),
    );
    expect(applied.some((t) => t.target_type === 'class' && t.target_id === 'Architect')).toBe(false);
  });

  it('disables the removed chip button while the removal is in flight, and re-enables it once settled', async () => {
    let resolveApply: () => void = () => {};
    const onApply = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveApply = resolve;
        }),
    );
    const deployments: DeploymentTarget[] = [{ target_type: 'class', target_id: 'Architect', linked: true }];
    render(<DeployTargetsControl entry={skillEntry()} deployments={deployments} onApply={onApply} />);

    const removeButton = screen.getByTestId('deploy-chip-remove-class:Architect');
    fireEvent.click(removeButton);
    expect(removeButton).toBeDisabled();

    resolveApply();
    await waitFor(() => expect(removeButton).not.toBeDisabled());
  });

  it('an onApply rejection does not crash the control and still clears the pending state', async () => {
    const onApply = vi.fn().mockRejectedValue(new Error('boom'));
    const deployments: DeploymentTarget[] = [{ target_type: 'class', target_id: 'Architect', linked: true }];
    render(<DeployTargetsControl entry={skillEntry()} deployments={deployments} onApply={onApply} />);

    const removeButton = screen.getByTestId('deploy-chip-remove-class:Architect');
    fireEvent.click(removeButton);

    await waitFor(() => expect(removeButton).not.toBeDisabled());
    // Still rendered, no crash.
    expect(screen.getByTestId('deploy-targets-control')).toBeInTheDocument();
  });

  describe('add-target picker', () => {
    it('opens on click, listing available targets grouped USER / CLASSES / AGENTS with counts', async () => {
      render(<DeployTargetsControl entry={skillEntry()} deployments={[]} onApply={vi.fn().mockResolvedValue(undefined)} />);

      fireEvent.click(screen.getByTestId('deploy-targets-add-button'));
      const picker = await screen.findByTestId('deploy-picker');
      expect(picker).toBeInTheDocument();

      expect(screen.getByTestId('deploy-picker-group-user')).toHaveTextContent('USER (1)');
      expect(screen.getByTestId('deploy-picker-group-class')).toHaveTextContent('CLASSES (2)');
      await waitFor(() => expect(screen.getByTestId('deploy-picker-group-agent')).toHaveTextContent('AGENTS (1)'));

      expect(screen.getByTestId('deploy-picker-option-user:global')).toHaveTextContent('User (global)');
      expect(screen.getByTestId('deploy-picker-option-class:Architect')).toHaveTextContent('Architect');
      expect(screen.getByTestId('deploy-picker-option-class:Coder')).toHaveTextContent('Coder');
      await waitFor(() =>
        expect(screen.getByTestId('deploy-picker-option-agent:agent-1')).toHaveTextContent('Coder One'),
      );
    });

    it('hides already-deployed targets from the picker', async () => {
      const deployments: DeploymentTarget[] = [{ target_type: 'class', target_id: 'Architect', linked: true }];
      render(<DeployTargetsControl entry={skillEntry()} deployments={deployments} onApply={vi.fn().mockResolvedValue(undefined)} />);

      fireEvent.click(screen.getByTestId('deploy-targets-add-button'));
      await screen.findByTestId('deploy-picker');

      expect(screen.queryByTestId('deploy-picker-option-class:Architect')).not.toBeInTheDocument();
      expect(screen.getByTestId('deploy-picker-option-class:Coder')).toBeInTheDocument();
    });

    it('filters options by the search query across groups', async () => {
      render(<DeployTargetsControl entry={skillEntry()} deployments={[]} onApply={vi.fn().mockResolvedValue(undefined)} />);
      fireEvent.click(screen.getByTestId('deploy-targets-add-button'));
      await screen.findByTestId('deploy-picker');
      await waitFor(() => expect(screen.getByTestId('deploy-picker-option-agent:agent-1')).toBeInTheDocument());

      fireEvent.change(screen.getByTestId('deploy-picker-search'), { target: { value: 'coder' } });

      expect(screen.queryByTestId('deploy-picker-option-user:global')).not.toBeInTheDocument();
      expect(screen.queryByTestId('deploy-picker-option-class:Architect')).not.toBeInTheDocument();
      expect(screen.getByTestId('deploy-picker-option-class:Coder')).toBeInTheDocument();
      expect(screen.getByTestId('deploy-picker-option-agent:agent-1')).toBeInTheDocument(); // "Coder One"
    });

    it('selecting a row deploys to that target, calling onApply with the full new set', async () => {
      const onApply = vi.fn().mockResolvedValue(undefined);
      const deployments: DeploymentTarget[] = [{ target_type: 'user', target_id: 'global', linked: true }];
      render(<DeployTargetsControl entry={skillEntry()} deployments={deployments} onApply={onApply} />);

      fireEvent.click(screen.getByTestId('deploy-targets-add-button'));
      await screen.findByTestId('deploy-picker');

      fireEvent.click(screen.getByTestId('deploy-picker-option-class:Architect'));

      expect(onApply).toHaveBeenCalledWith(
        expect.arrayContaining([
          { target_type: 'user', target_id: 'global' },
          { target_type: 'class', target_id: 'Architect' },
        ]),
      );
      const applied = onApply.mock.calls[0][0] as SkillDeployment[];
      expect(applied).toHaveLength(2);
    });

    it('supports arrow-key navigation and Enter to select', async () => {
      const onApply = vi.fn().mockResolvedValue(undefined);
      render(<DeployTargetsControl entry={skillEntry()} deployments={[]} onApply={onApply} />);

      fireEvent.click(screen.getByTestId('deploy-targets-add-button'));
      const search = await screen.findByTestId('deploy-picker-search');

      // Flat order is USER, CLASSES(Architect, Coder), AGENTS — two
      // ArrowDown presses from the default (index 0, User) land on Coder.
      fireEvent.keyDown(search, { key: 'ArrowDown' });
      fireEvent.keyDown(search, { key: 'ArrowDown' });
      fireEvent.keyDown(search, { key: 'Enter' });

      expect(onApply).toHaveBeenCalledWith([{ target_type: 'class', target_id: 'Coder' }]);
    });

    it('closes on Escape', async () => {
      render(<DeployTargetsControl entry={skillEntry()} deployments={[]} onApply={vi.fn().mockResolvedValue(undefined)} />);
      fireEvent.click(screen.getByTestId('deploy-targets-add-button'));
      const search = await screen.findByTestId('deploy-picker-search');

      fireEvent.keyDown(search, { key: 'Escape' });
      expect(screen.queryByTestId('deploy-picker')).not.toBeInTheDocument();
    });

    it('closes on outside click', async () => {
      render(
        <div>
          <div data-testid="outside">outside</div>
          <DeployTargetsControl entry={skillEntry()} deployments={[]} onApply={vi.fn().mockResolvedValue(undefined)} />
        </div>,
      );
      fireEvent.click(screen.getByTestId('deploy-targets-add-button'));
      await screen.findByTestId('deploy-picker');

      fireEvent.mouseDown(screen.getByTestId('outside'));
      expect(screen.queryByTestId('deploy-picker')).not.toBeInTheDocument();
    });
  });

  describe('concurrent ops (deploy-redesign-review.md C1: fresh-state accumulation + serialization)', () => {
    /** Builds an `onApply` mock whose calls never auto-settle — each call
     * pushes a `{ resolve, reject }` pair onto `defers` so the test can
     * decide exactly when each queued op "completes", to drive races
     * deterministically instead of relying on real timing. */
    function deferredOnApply() {
      const defers: Array<{ resolve: () => void; reject: (e: unknown) => void }> = [];
      const onApply = vi.fn(
        (_targets: SkillDeployment[]) =>
          new Promise<void>((resolve, reject) => {
            defers.push({ resolve, reject });
          }),
      );
      return { onApply, defers };
    }

    it('two rapid adds without waiting: both targets survive in the final invoke payload, nothing reverted', async () => {
      const { onApply, defers } = deferredOnApply();
      render(<DeployTargetsControl entry={skillEntry()} deployments={[]} onApply={onApply} />);

      fireEvent.click(screen.getByTestId('deploy-targets-add-button'));
      await screen.findByTestId('deploy-picker');
      await waitFor(() => expect(screen.getByTestId('deploy-picker-option-agent:agent-1')).toBeInTheDocument());

      // Fire both adds back-to-back, without awaiting anything in between.
      fireEvent.click(screen.getByTestId('deploy-picker-option-class:Architect'));
      fireEvent.click(screen.getByTestId('deploy-picker-option-class:Coder'));

      // Only the first op has actually reached onApply so far — the second
      // is queued behind it (serialization).
      await waitFor(() => expect(onApply).toHaveBeenCalledTimes(1));
      expect(onApply.mock.calls[0][0]).toEqual([{ target_type: 'class', target_id: 'Architect' }]);

      defers[0].resolve();
      await waitFor(() => expect(onApply).toHaveBeenCalledTimes(2));
      // Fresh-state accumulation: the second call's desired set was
      // computed from the local desired-set ref, which already included
      // Architect at the time Coder was clicked — so both survive, instead
      // of the second call's `desired` reverting to a stale two-item-ago
      // snapshot that omits Architect.
      expect(onApply.mock.calls[1][0]).toEqual(
        expect.arrayContaining([
          { target_type: 'class', target_id: 'Architect' },
          { target_type: 'class', target_id: 'Coder' },
        ]),
      );
      expect(onApply.mock.calls[1][0]).toHaveLength(2);

      defers[1].resolve();
      await waitFor(() => expect(screen.getByTestId('deploy-picker-option-class:Coder')).not.toBeDisabled());
    });

    it('add then immediately remove a different target: both changes survive', async () => {
      const { onApply, defers } = deferredOnApply();
      const deployments: DeploymentTarget[] = [{ target_type: 'class', target_id: 'Architect', linked: true }];
      render(<DeployTargetsControl entry={skillEntry()} deployments={deployments} onApply={onApply} />);

      fireEvent.click(screen.getByTestId('deploy-targets-add-button'));
      await screen.findByTestId('deploy-picker');
      await waitFor(() => expect(screen.getByTestId('deploy-picker-option-class:Coder')).toBeInTheDocument());

      fireEvent.click(screen.getByTestId('deploy-picker-option-class:Coder')); // add Coder
      fireEvent.click(screen.getByTestId('deploy-chip-remove-class:Architect')); // remove Architect

      await waitFor(() => expect(onApply).toHaveBeenCalledTimes(1));
      expect(onApply.mock.calls[0][0]).toEqual(
        expect.arrayContaining([
          { target_type: 'class', target_id: 'Architect' },
          { target_type: 'class', target_id: 'Coder' },
        ]),
      );
      expect(onApply.mock.calls[0][0]).toHaveLength(2);

      defers[0].resolve();
      await waitFor(() => expect(onApply).toHaveBeenCalledTimes(2));
      // The queued remove computed its desired set from the local ref
      // *after* the add had already applied its delta — so Architect is
      // gone and Coder (added moments earlier) survives.
      expect(onApply.mock.calls[1][0]).toEqual([{ target_type: 'class', target_id: 'Coder' }]);

      defers[1].resolve();
    });

    it('serializes queued ops: the second invoke is not issued until the first settles', async () => {
      const { onApply, defers } = deferredOnApply();
      const deployments: DeploymentTarget[] = [
        { target_type: 'class', target_id: 'Architect', linked: true },
        { target_type: 'class', target_id: 'Coder', linked: true },
      ];
      render(<DeployTargetsControl entry={skillEntry()} deployments={deployments} onApply={onApply} />);

      fireEvent.click(screen.getByTestId('deploy-chip-remove-class:Architect'));
      fireEvent.click(screen.getByTestId('deploy-chip-remove-class:Coder'));

      await waitFor(() => expect(onApply).toHaveBeenCalledTimes(1));
      // Give any wrongly-unserialized second call a chance to fire, then
      // confirm it still hasn't.
      await new Promise((resolve) => setTimeout(resolve, 0));
      expect(onApply).toHaveBeenCalledTimes(1);

      defers[0].resolve();
      await waitFor(() => expect(onApply).toHaveBeenCalledTimes(2));

      defers[1].resolve();
    });

    it('a failure mid-queue re-syncs the local set from props, so no phantom chip lingers on the next op', async () => {
      const defers: Array<{ resolve: () => void; reject: (e: unknown) => void }> = [];
      const onApply = vi.fn(
        (_targets: SkillDeployment[]) =>
          new Promise<void>((resolve, reject) => {
            defers.push({ resolve, reject });
          }),
      );
      const deployments: DeploymentTarget[] = [{ target_type: 'user', target_id: 'global', linked: true }];
      render(<DeployTargetsControl entry={skillEntry()} deployments={deployments} onApply={onApply} />);

      fireEvent.click(screen.getByTestId('deploy-targets-add-button'));
      await screen.findByTestId('deploy-picker');
      await waitFor(() => expect(screen.getByTestId('deploy-picker-option-class:Architect')).toBeInTheDocument());

      fireEvent.click(screen.getByTestId('deploy-picker-option-class:Architect')); // will fail
      await waitFor(() => expect(onApply).toHaveBeenCalledTimes(1));
      expect(onApply.mock.calls[0][0]).toEqual(
        expect.arrayContaining([
          { target_type: 'user', target_id: 'global' },
          { target_type: 'class', target_id: 'Architect' },
        ]),
      );

      defers[0].reject(new Error('boom'));
      await waitFor(() => expect(screen.getByTestId('deploy-picker-option-class:Architect')).not.toBeDisabled());

      // Add a different target next. If the local set hadn't re-synced
      // after the failure, this would compound onto the abandoned
      // optimistic state and still include the failed Architect add; it
      // must not.
      fireEvent.click(screen.getByTestId('deploy-picker-option-class:Coder'));
      await waitFor(() => expect(onApply).toHaveBeenCalledTimes(2));
      expect(onApply.mock.calls[1][0]).toEqual(
        expect.arrayContaining([
          { target_type: 'user', target_id: 'global' },
          { target_type: 'class', target_id: 'Coder' },
        ]),
      );
      expect(onApply.mock.calls[1][0]).toHaveLength(2);

      defers[1].resolve();
    });
  });

  it('accepts a drop of a different skill ref and switches selection to it', async () => {
    const select = vi.fn().mockResolvedValue(undefined);
    useLibraryStore.setState({ select });
    render(<DeployTargetsControl entry={skillEntry()} deployments={[]} onApply={vi.fn().mockResolvedValue(undefined)} />);

    const dataTransfer = { getData: () => 'skills/other/tool' } as unknown as DataTransfer;
    fireEvent.drop(screen.getByTestId('deploy-targets-control'), { dataTransfer });

    expect(select).toHaveBeenCalledWith('skills/other/tool');
  });
});
