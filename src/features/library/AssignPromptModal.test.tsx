import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { invoke } from '@tauri-apps/api/core';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AgentConfig, LibraryPrompt } from '../../types';
import { submitInputToAgent } from '../../utils/terminalInput';
import { AssignPromptModal } from './AssignPromptModal';

vi.mock('../../utils/terminalInput', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../utils/terminalInput')>();
  return {
    ...actual,
    submitInputToAgent: vi.fn(),
  };
});

const mockInvoke = vi.mocked(invoke);
const mockSubmitInputToAgent = vi.mocked(submitInputToAgent);

const prompt: LibraryPrompt = {
  type: 'Prompt',
  path: 'prompts/review.md',
  name: 'Review Prompt',
  content: '# Review\n\nCheck the implementation.',
  metadata: { id: 'prompt-1', tags: [], is_starred: false },
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

describe('AssignPromptModal', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockSubmitInputToAgent.mockResolvedValue(undefined);
  });

  it('loads active agents and runs the prompt against the selected agent', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    mockInvoke.mockResolvedValue([
      agent(),
      agent({ session_id: 'agent-2', session_name: 'Reviewer Two', agent_class: 'Reviewer' }),
    ]);

    render(<AssignPromptModal prompt={prompt} isOpen onClose={onClose} />);

    expect(await screen.findByRole('option', { name: 'Coder One (Coder)' })).toBeInTheDocument();
    await user.selectOptions(screen.getByRole('combobox'), 'agent-2');
    await user.click(screen.getByRole('button', { name: 'Run Prompt' }));

    expect(mockSubmitInputToAgent).toHaveBeenCalledWith(
      'agent-2',
      '# Review  Check the implementation.',
    );
    expect(onClose).toHaveBeenCalled();
  });

  it('disables running when no active agents are available', async () => {
    mockInvoke.mockResolvedValue([]);

    render(<AssignPromptModal prompt={prompt} isOpen onClose={() => {}} />);

    expect(await screen.findByRole('option', { name: 'No active agents' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Run Prompt' })).toBeDisabled();
  });

  it('does not render while closed', () => {
    render(<AssignPromptModal prompt={prompt} isOpen={false} onClose={() => {}} />);

    expect(screen.queryByText('Run Review Prompt')).not.toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it('keeps the modal open and reports injection errors', async () => {
    const user = userEvent.setup();
    const alertSpy = vi.spyOn(window, 'alert').mockImplementation(() => {});
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    const onClose = vi.fn();
    mockInvoke.mockResolvedValue([agent()]);
    mockSubmitInputToAgent.mockRejectedValue(new Error('terminal unavailable'));

    render(<AssignPromptModal prompt={prompt} isOpen onClose={onClose} />);

    await screen.findByRole('option', { name: 'Coder One (Coder)' });
    await user.click(screen.getByRole('button', { name: 'Run Prompt' }));

    await waitFor(() => {
      expect(alertSpy).toHaveBeenCalledWith(expect.stringContaining('terminal unavailable'));
    });
    expect(onClose).not.toHaveBeenCalled();

    alertSpy.mockRestore();
    consoleError.mockRestore();
  });
});
