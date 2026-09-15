import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import type { AgentChatEvent } from '../../types';
import { ChatTranscriptRow } from './ChatTranscriptRows';
import { derivePresentedChatRows } from '../grid/workLogPresentation';

const memoryEvent: AgentChatEvent = {
  id: 'memory:event-1',
  session_id: 'agent-1',
  provider: 'wardian',
  kind: 'memory',
  role: 'system',
  text: '# Wardian memory\n\n- Prefer compact layouts',
  title: 'Memory loaded',
  status: 'succeeded',
  turn_id: null,
  source: 'wardian_memory',
  command: null,
  exit_code: null,
  path: null,
  language: 'markdown',
  created_at: '2026-08-23T12:00:00Z',
  sequence: 1,
  metadata: { memory_action: 'loaded' },
};

describe('memory transcript row', () => {
  it('is compact by default and reveals the exact injected context', () => {
    render(
      <ChatTranscriptRow
        agentIsWorking={false}
        isSubmitting={false}
        onApprovalSubmit={vi.fn()}
        row={{ kind: 'event', event: memoryEvent }}
      />,
    );

    expect(screen.getByText('Memory loaded')).toBeInTheDocument();
    expect(screen.queryByText('Prefer compact layouts')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /Memory loaded/ }));
    expect(screen.getByText('Prefer compact layouts')).toBeInTheDocument();
  });

  it('applies the remote file policy to expanded memory markdown', () => {
    const openUrl = vi.fn().mockRejectedValue(new Error('browser-only opener rejected file URL'));
    const openWindow = vi.spyOn(window, 'open').mockReturnValue({} as Window);

    render(
      <ChatTranscriptRow
        agentIsWorking={false}
        isSubmitting={false}
        linkHandling={{ allowFileLinks: false, openUrl }}
        onApprovalSubmit={vi.fn()}
        row={{
          kind: 'event',
          event: {
            ...memoryEvent,
            text: 'Open [host memory](file:///C:/host/memory.md) and ![host image](file:///C:/host/image.png).',
          },
        }}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: /Memory loaded/ }));

    expect(screen.getByText('host memory')).toBeInTheDocument();
    expect(screen.getByText('host image')).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'host memory' })).not.toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'file:///C:/host/image.png' })).not.toBeInTheDocument();
    expect(openUrl).not.toHaveBeenCalled();
    expect(openWindow).not.toHaveBeenCalled();

    openWindow.mockRestore();
  });
});

describe('compact tool-call transcript row', () => {
  it('shows the actual command when the provider title is only an exec lifecycle label', () => {
    const event: AgentChatEvent = {
      ...memoryEvent,
      id: 'exec-call-1',
      kind: 'tool_call',
      role: null,
      text: null,
      title: 'exec_command_begin',
      status: 'running',
      command: 'npm test',
      language: 'shell',
      sequence: 1,
      metadata: { raw_type: 'exec_command_begin' },
    };
    const rows = derivePresentedChatRows([event]);
    const row = rows[0];

    if (row.kind !== 'event') throw new Error('expected event row');
    render(
      <ChatTranscriptRow
        agentIsWorking
        isSubmitting={false}
        onApprovalSubmit={vi.fn()}
        row={row}
      />,
    );

    const summary = screen.getByTestId('chat-tool-call-summary');
    expect(summary).toHaveTextContent('$ npm test');
    expect(summary).not.toHaveTextContent('exec command begin');
    expect(summary).not.toHaveTextContent('No activity content');
    expect(summary).not.toHaveTextContent('1 line');
  });

  it('keeps a lifecycle-labelled approval in the actionable approval surface', () => {
    const event: AgentChatEvent = {
      ...memoryEvent,
      id: 'exec-approval-1',
      kind: 'tool_call',
      role: null,
      text: 'Do you want to proceed?\n> 1. Yes\n> 2. No',
      title: 'exec_command_begin',
      status: 'action_required',
      command: 'npm test',
      language: 'shell',
      sequence: 1,
      metadata: { raw_type: 'exec_command_begin' },
    };
    const rows = derivePresentedChatRows([event]);
    const row = rows[0];

    if (row.kind !== 'event') throw new Error('expected event row');
    render(
      <ChatTranscriptRow
        agentIsWorking={false}
        isSubmitting={false}
        liveApprovalId={event.id}
        onApprovalSubmit={vi.fn()}
        row={row}
      />,
    );

    expect(screen.queryByTestId('chat-tool-call-summary')).toBeNull();
    expect(screen.getByTestId('chat-approval-notice')).toHaveTextContent('Action required');
  });
});
