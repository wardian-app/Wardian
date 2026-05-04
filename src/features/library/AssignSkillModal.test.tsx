import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { invoke } from '@tauri-apps/api/core';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AgentClassDefinition, AgentConfig, LibrarySkill } from '../../types';
import { useLibraryStore } from '../../store/useLibraryStore';
import { AssignSkillModal } from './AssignSkillModal';

const mockInvoke = vi.mocked(invoke);

const skill: LibrarySkill = {
  type: 'Skill',
  path: 'skills/planner',
  name: 'planner',
  description: '# planner',
  content: '# planner',
  metadata: { id: 'skill-1', tags: [], is_starred: false },
};

const agent = (overrides: Partial<AgentConfig> = {}): AgentConfig => ({
  session_id: 'agent-1',
  session_name: 'Coder One',
  agent_class: 'Coder',
  folder: 'C:/work/app',
  is_off: false,
  provider: 'mock',
  ...overrides,
});

const agentClass = (overrides: Partial<AgentClassDefinition> = {}): AgentClassDefinition => ({
  name: 'Coder',
  description: 'Writes code',
  is_default: true,
  ...overrides,
});

describe('AssignSkillModal', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useLibraryStore.setState({
      promptTree: null,
      skillTree: null,
      isLoading: false,
      error: null,
      activeTab: 'skills',
    });
    mockInvoke.mockImplementation(async (command, args) => {
      if (command === 'list_agents') return [agent()];
      if (command === 'list_agent_classes') return [agentClass(), agentClass({ name: 'Reviewer' })];
      if (command === 'list_skill_deployments') {
        expect(args).toEqual({ skillName: 'planner', sourcePath: 'skills/planner' });
        return [{ target_type: 'agent', target_id: 'agent-1' }];
      }
      return undefined;
    });
  });

  it('loads deployments and deploys to the selected class target', async () => {
    const user = userEvent.setup();

    render(<AssignSkillModal skill={skill} isOpen onClose={() => {}} />);

    expect(await screen.findByText('Coder One')).toBeInTheDocument();
    await user.selectOptions(screen.getByLabelText('Target Scope'), 'class');
    expect(screen.getByLabelText('Select Class')).toHaveValue('Coder');
    await user.selectOptions(screen.getByLabelText('Select Class'), 'Reviewer');
    await user.click(screen.getByRole('button', { name: 'Deploy Skill' }));

    expect(mockInvoke).toHaveBeenCalledWith('deploy_skill', {
      sourcePath: 'skills/planner',
      targetType: 'class',
      targetId: 'Reviewer',
    });
  });

  it('defaults agent scope to the first active agent and removes deployments', async () => {
    const user = userEvent.setup();

    render(<AssignSkillModal skill={skill} isOpen onClose={() => {}} />);

    expect(await screen.findByText('Coder One')).toBeInTheDocument();
    await user.selectOptions(screen.getByLabelText('Target Scope'), 'agent');
    expect(screen.getByLabelText('Select Agent')).toHaveValue('agent-1');
    await user.click(screen.getByRole('button', { name: 'Deploy Skill' }));

    expect(mockInvoke).toHaveBeenCalledWith('deploy_skill', {
      sourcePath: 'skills/planner',
      targetType: 'agent',
      targetId: 'agent-1',
    });

    await user.click(screen.getByTitle('Remove Deployment'));
    expect(mockInvoke).toHaveBeenCalledWith('remove_deployed_skill', {
      targetType: 'agent',
      targetId: 'agent-1',
      skillName: 'planner',
    });
  });

  it('shows empty target options and disables deploy when class list is empty', async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'list_agents') return [];
      if (command === 'list_agent_classes') return [];
      if (command === 'list_skill_deployments') return [];
      return undefined;
    });
    const user = userEvent.setup();

    render(<AssignSkillModal skill={skill} isOpen onClose={() => {}} />);

    expect(await screen.findByText('Skill is not deployed anywhere.')).toBeInTheDocument();
    await user.selectOptions(screen.getByLabelText('Target Scope'), 'class');

    expect(screen.getByRole('option', { name: 'No custom classes available' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Deploy Skill' })).toBeDisabled();
  });

  it('does not render while closed', () => {
    render(<AssignSkillModal skill={skill} isOpen={false} onClose={() => {}} />);

    expect(screen.queryByText('Manage planner')).not.toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it('surfaces deploy errors without closing the modal', async () => {
    const user = userEvent.setup();
    const alertSpy = vi.spyOn(window, 'alert').mockImplementation(() => {});
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'list_agents') return [agent()];
      if (command === 'list_agent_classes') return [agentClass()];
      if (command === 'list_skill_deployments') return [];
      if (command === 'deploy_skill') throw new Error('deploy failed');
      return undefined;
    });

    render(<AssignSkillModal skill={skill} isOpen onClose={() => {}} />);

    await screen.findByRole('button', { name: 'Deploy Skill' });
    await user.click(screen.getByRole('button', { name: 'Deploy Skill' }));

    await waitFor(() => {
      expect(alertSpy).toHaveBeenCalledWith(expect.stringContaining('deploy failed'));
    });
    expect(screen.getByText('Manage planner')).toBeInTheDocument();

    alertSpy.mockRestore();
    consoleError.mockRestore();
  });
});
