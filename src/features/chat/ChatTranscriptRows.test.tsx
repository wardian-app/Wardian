import { fireEvent, render, screen, waitFor } from '@testing-library/react';
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
  it('shows missing saved details without an indefinite pending state or a load', () => {
    const load = vi.fn();
    render(<ChatTranscriptRow agentIsWorking={false} isSubmitting={false} onApprovalSubmit={vi.fn()} onLoadDetail={load}
      row={{ kind: 'event', event: { ...memoryEvent, kind: 'message', text: 'saved preview',
        metadata: { chat_body_unavailable: true, chat_body_pending: true } } }} />);
    expect(screen.getByRole('status')).toHaveTextContent('Saved details are unavailable.');
    expect(screen.queryByText('Details are updating.')).toBeNull();
    expect(screen.queryByRole('button', { name: 'Show full details' })).toBeNull();
    expect(load).not.toHaveBeenCalled();
  });

  it('preserves opened chunks when the remaining saved body becomes unavailable', async () => {
    const event: AgentChatEvent = { ...memoryEvent, kind: 'message', text: 'preview',
      metadata: { chat_detail_ref: 'prefix:0', chat_body_binding: 'same-body', chat_body_pending: true } };
    const load = vi.fn().mockResolvedValueOnce({ event_id: event.id, text: 'committed prefix', next: 'prefix:16', complete: false })
      .mockResolvedValueOnce({ event_id: event.id, text: ' more prefix', next: null, complete: false });
    const props = { agentIsWorking: false, isSubmitting: false, onApprovalSubmit: vi.fn(), onLoadDetail: load };
    const view = render(<ChatTranscriptRow {...props} row={{ kind: 'event', event }} />);
    fireEvent.click(screen.getByRole('button', { name: 'Show full details' }));
    await waitFor(() => expect(view.container.querySelector('pre')).toHaveTextContent('committed prefix'));
    view.rerender(<ChatTranscriptRow {...props} row={{ kind: 'event', event: { ...event,
      metadata: { ...event.metadata, chat_body_pending: false, chat_body_unavailable: true } } }} />);
    expect(view.container.querySelector('pre')).toHaveTextContent('committed prefix');
    fireEvent.click(screen.getByRole('button', { name: 'Read more' }));
    await waitFor(() => expect(view.container.querySelector('pre')).toHaveTextContent('committed prefix more prefix'));
    expect(screen.getByRole('status')).toHaveTextContent('Saved details are unavailable.');
    expect(screen.queryByText('Details are updating.')).toBeNull();
    expect(screen.queryByRole('button', { name: 'Read more' })).toBeNull();
    expect(load).toHaveBeenCalledTimes(2);
  });

  it('keeps missing-body errors sanitized and distinguishes pending details', async () => {
    const load = vi.fn().mockRejectedValue(new Error('missing private/owned/artifact.txt'));
    const props = { agentIsWorking: false, isSubmitting: false, onApprovalSubmit: vi.fn(), onLoadDetail: load };
    const pending: AgentChatEvent = { ...memoryEvent, kind: 'message', text: 'preview', metadata: { chat_body_pending: true } };
    const view = render(<ChatTranscriptRow {...props} row={{ kind: 'event', event: pending }} />);
    expect(screen.getByText('Details are updating.')).toBeInTheDocument();
    view.rerender(<ChatTranscriptRow {...props} row={{ kind: 'event', event: { ...pending,
      metadata: { chat_body_unavailable: true, chat_detail_ref: 'saved-prefix:0' } } }} />);
    fireEvent.click(screen.getByRole('button', { name: 'Show full details' }));
    await waitFor(() => expect(load).toHaveBeenCalledTimes(1));
    expect(screen.getByRole('status')).toHaveTextContent('Saved details are unavailable.');
    expect(view.container).not.toHaveTextContent('private/owned/artifact.txt');
    expect(view.container).not.toHaveTextContent('Details are updating.');
  });

  it('keeps opened details across a verified replacement and avoids repeating the first chunk', async () => {
    const observation: AgentChatEvent = { ...memoryEvent, id: 'observation', kind: 'message', role: 'user', text: 'preview',
      metadata: { chat_display_key: 'slot', chat_detail_ref: 'observation:0', chat_body_binding: 'body' } };
    const load = vi.fn().mockResolvedValueOnce({ event_id: 'observation', text: 'body', next: null, complete: true })
      .mockResolvedValueOnce({ event_id: 'canonical', text: 'body-more', next: null, complete: true });
    const props = { agentIsWorking: false, isSubmitting: false, onApprovalSubmit: vi.fn(), onLoadDetail: load };
    const view = render(<ChatTranscriptRow {...props} row={{ kind: 'event', event: observation }} />);
    fireEvent.click(screen.getByRole('button', { name: 'Show full details' }));
    await waitFor(() => expect(view.container.querySelector('pre')).toHaveTextContent('body'));
    view.rerender(<ChatTranscriptRow {...props} row={{ kind: 'event', event: { ...observation, id: 'canonical',
      metadata: { ...observation.metadata, chat_detail_ref: 'canonical:0' } } }} />);
    expect(view.container.querySelector('pre')).toHaveTextContent('body');
    fireEvent.click(screen.getByRole('button', { name: 'Read more' }));
    await waitFor(() => expect(view.container.querySelector('pre')).toHaveTextContent('body-more'));
    expect(view.container.querySelector('pre')).not.toHaveTextContent('bodybody-more');
  });

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

describe('message transcript row actions', () => {
  it('keeps assistant copy actions in a leading side gutter', () => {
    render(
      <ChatTranscriptRow
        agentIsWorking={false}
        isSubmitting={false}
        onApprovalSubmit={vi.fn()}
        row={{
          kind: 'event',
          event: {
            ...memoryEvent,
            id: 'assistant-message-1',
            kind: 'message',
            role: 'assistant',
            text: 'Assistant response',
            title: null,
          },
        }}
      />,
    );

    const article = screen.getByRole('article', { name: 'assistant message' });
    const layout = article.querySelector('.chat-message-layout--assistant');

    expect(layout).toBeInTheDocument();
    expect(layout?.querySelector('.chat-message-content')).toBeInTheDocument();
    expect(layout?.querySelector('.chat-row-actions--inline')).toBeInTheDocument();
    expect(layout?.children).toHaveLength(2);
  });

  it('keeps user copy actions in a trailing side gutter', () => {
    render(
      <ChatTranscriptRow
        agentIsWorking={false}
        isSubmitting={false}
        onApprovalSubmit={vi.fn()}
        row={{
          kind: 'event',
          event: {
            ...memoryEvent,
            id: 'user-message-1',
            kind: 'message',
            role: 'user',
            text: 'User prompt',
            title: null,
          },
        }}
      />,
    );

    const article = screen.getByRole('article', { name: 'user message' });
    const layout = article.querySelector('.chat-message-layout--user');

    expect(layout).toBeInTheDocument();
    expect(layout?.querySelector('.chat-message-content')).toBeInTheDocument();
    expect(layout?.querySelector('.chat-row-actions--inline')).toBeInTheDocument();
    expect(layout?.children).toHaveLength(2);
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
