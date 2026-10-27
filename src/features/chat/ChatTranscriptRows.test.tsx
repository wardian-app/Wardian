import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import type { AgentChatDetail, AgentChatEvent, AgentChatPage } from '../../types';
import producerFixture from './__fixtures__/chat-row-revalidation-producer-consumer-order-r3.json';
import { ChatTranscriptRow } from './ChatTranscriptRows';
import { derivePresentedChatRows } from '../grid/workLogPresentation';
import { chatRevalidationIds } from './chatRevalidation';
import { applyRevalidationReplies } from '../../test/chatRevalidationTestHelpers';
import { applyChatPage } from './chatReadState';

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

const actualProducerPage = (label: string): AgentChatPage => {
  const page = (producerFixture.pages as Array<{ label: string; dto: AgentChatPage }>).find((candidate) => candidate.label === label);
  if (!page) throw new Error(`Missing producer fixture page: ${label}`);
  return page.dto;
};

const actualProducerPages = (prefix: string): AgentChatPage[] => (producerFixture.pages as Array<{ label: string; dto: AgentChatPage }>)
  .filter((candidate) => candidate.label.startsWith(prefix))
  .sort((left, right) => Number(left.label.slice(prefix.length)) - Number(right.label.slice(prefix.length)))
  .map((candidate) => candidate.dto);

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

  it('keeps the public A slot while replacing B body chunks with the actual renewed A source', async () => {
    const h0Recent = actualProducerPage('loaded-long-pair-H0-recent-full');
    const h0Older = actualProducerPage('loaded-long-pair-H0-older-full');
    const h1Older = actualProducerPage('loaded-pair-H1-first-older-full');
    const h2Older = actualProducerPage('loaded-pair-H2-first-older-after-late-C-full');
    const h2Batches = actualProducerPages('loaded-window-H2-recent-after-late-C-batch-');
    expect(h2Batches).toHaveLength(40);

    const loadedWindow = [h0Recent, h0Older, h1Older, h2Older]
      .reduce<AgentChatEvent[]>((events, page, index) =>
        applyChatPage(events, page, index === 0 ? 'recent' : 'older'), []);
    const requestedIds = chatRevalidationIds(loadedWindow);
    const acknowledgedIds = h2Batches.flatMap((page) => page.revalidated_ids ?? []);
    expect(requestedIds).toHaveLength(320);
    expect(acknowledgedIds).toHaveLength(requestedIds.length);
    expect(new Set(acknowledgedIds)).toEqual(new Set(requestedIds));

    const publicA = actualProducerPage('loaded-pair-H1-A-only').events.find((event) =>
      Array.isArray(event.metadata.chat_display_member_ids) && event.metadata.chat_display_member_ids.length === 2);
    if (!publicA) throw new Error('The captured H1 A-only response omitted the public A pair row.');
    const memberIds = Array.isArray(publicA.metadata.chat_display_member_ids)
      ? publicA.metadata.chat_display_member_ids.filter((id): id is string => typeof id === 'string') : [];
    expect(memberIds).toHaveLength(2);
    const returnedRows = h2Batches.flatMap((page) => page.events);
    const rawA = returnedRows.find((event) => event.id === publicA.id);
    const rawB = returnedRows.find((event) => event.id !== publicA.id
      && event.metadata.chat_source_ref === memberIds[1]);
    if (!rawA || !rawB) throw new Error('The actual H2 batch omitted one of the pair members.');
    expect(publicA.id).toBe(memberIds[0]);
    expect(publicA.metadata.chat_source_ref).toBe(memberIds[1]);
    expect(publicA.metadata.chat_display_physical_id).toBe(memberIds[1]);
    expect(rawA.metadata.chat_source_ref).toBe(publicA.id);
    expect(publicA.metadata.chat_body_binding).toBeUndefined();
    expect(rawA.metadata.chat_body_binding).toBeUndefined();
    expect(typeof publicA.metadata.chat_detail_ref).toBe('string');
    expect(typeof rawA.metadata.chat_detail_ref).toBe('string');

    const renewed = applyRevalidationReplies(loadedWindow, h2Batches, 'older');
    const renewedA = renewed.find((event) => event.id === publicA.id);
    if (!renewedA) throw new Error('The actual revalidation reply omitted raw A.');
    expect(renewedA.metadata.chat_body_binding).toBeUndefined();
    expect(publicA.metadata.chat_display_key ?? publicA.id).toBe(publicA.id);
    expect(renewedA.metadata.chat_display_key).toBe(publicA.id);
    expect(renewedA.metadata.chat_source_ref).toBe(rawA.metadata.chat_source_ref);
    expect(renewed).toContainEqual(expect.objectContaining({ id: rawB.id, metadata: expect.objectContaining({
      chat_source_ref: rawB.metadata.chat_source_ref,
    }) }));

    let resolveLateChunk!: (detail: AgentChatDetail) => void;
    const lateChunk = new Promise<AgentChatDetail>((resolve) => { resolveLateChunk = resolve; });
    const oldReference = String(publicA.metadata.chat_detail_ref);
    const renewedReference = String(renewedA.metadata.chat_detail_ref);
    expect(renewedReference).not.toBe(oldReference);
    const load = vi.fn((reference: string): Promise<AgentChatDetail> => {
      if (reference === oldReference) return Promise.resolve({
        event_id: publicA.id, text: 'old B body already opened', next: 'old-B:next', complete: false,
      });
      if (reference === 'old-B:next') return lateChunk;
      if (reference === renewedReference) return Promise.resolve({
        event_id: renewedA.id, text: 'current A owned body', next: null, complete: true,
      });
      return Promise.reject(new Error('unexpected detail reference'));
    });
    const rowProps = { agentIsWorking: false, isSubmitting: false, onApprovalSubmit: vi.fn(), onLoadDetail: load };
    const view = render(<ChatTranscriptRow {...rowProps} row={{ kind: 'event', event: publicA }} />);
    const stableMessageNode = view.container.querySelector('article[aria-label="assistant message"]');
    fireEvent.click(screen.getByRole('button', { name: 'Show full details' }));
    await waitFor(() => expect(view.container.querySelector('pre')).toHaveTextContent('old B body already opened'));
    fireEvent.click(screen.getByRole('button', { name: 'Read more' }));
    await waitFor(() => expect(load).toHaveBeenNthCalledWith(2, 'old-B:next'));

    view.rerender(<ChatTranscriptRow {...rowProps} row={{ kind: 'event', event: renewedA }} />);
    expect(view.container.querySelector('article[aria-label="assistant message"]')).toBe(stableMessageNode);
    expect(view.container.querySelector('pre')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Show full details' }));
    await waitFor(() => expect(view.container.querySelector('pre')).toHaveTextContent('current A owned body'));
    expect(load).toHaveBeenNthCalledWith(3, renewedReference);

    await act(async () => {
      resolveLateChunk({ event_id: publicA.id, text: 'late B body must not append', next: null, complete: true });
      await lateChunk;
    });
    expect(view.container.querySelector('pre')).toHaveTextContent('current A owned body');
    expect(view.container.querySelector('pre')).not.toHaveTextContent('late B body');
  });
  it('opens an actual unbound renewed A header using its own source reference', async () => {
    const initialPair = actualProducerPage('loaded-pair-H1-A-only').events.find((event) =>
      Array.isArray(event.metadata.chat_display_member_ids) && event.metadata.chat_display_member_ids.length === 2);
    if (!initialPair) throw new Error('The captured H1 A-only response omitted the paired public A row.');
    const rawA = actualProducerPages('loaded-window-H2-recent-after-late-C-batch-')
      .flatMap((page) => page.events)
      .find((event) => event.id === initialPair.id && event.metadata.chat_source_ref === initialPair.id);
    if (!rawA) throw new Error('The captured H2 revalidation pages omitted raw A with its own source reference.');

    expect(rawA.metadata.chat_body_binding).toBeUndefined();
    expect(rawA.metadata.chat_source_ref).toBe(rawA.id);
    expect(typeof rawA.metadata.chat_detail_ref).toBe('string');

    const load = vi.fn().mockResolvedValue({
      event_id: rawA.id, text: 'renewed A owned body', next: null, complete: true,
    });
    render(<ChatTranscriptRow agentIsWorking={false} isSubmitting={false} onApprovalSubmit={vi.fn()}
      onLoadDetail={load} row={{ kind: 'event', event: rawA }} />);
    fireEvent.click(screen.getByRole('button', { name: 'Show full details' }));
    await waitFor(() => expect(screen.getByText('renewed A owned body')).toBeInTheDocument());
    expect(load).toHaveBeenCalledWith(rawA.metadata.chat_detail_ref);
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
