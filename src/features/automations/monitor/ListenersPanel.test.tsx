import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ListenerView } from '../../../types/automation';
import { useListenersStore } from '../../../store/useListenersStore';
import { ListenersPanel } from './ListenersPanel';

const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invokeMock(...args) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => undefined) }));

const webhookListener: ListenerView = {
  id: 'saved-hook',
  blueprint_id: 'audit',
  name: 'CI hook',
  enabled: false,
  trigger: {
    type: 'webhook',
    path_segment: 'ci',
    auth: 'hmac_sha256',
    signature_header: null,
    max_body_bytes: 262144,
  },
  input: {},
  bindings: {},
  has_secret: false,
  webhook_url: 'http://127.0.0.1:8787/hooks/ci',
  runtime: { armed: false, fire_count: 0, recent_fire_epoch_ms: [], consecutive_failures: 0 },
};

const assignedListener: ListenerView = {
  ...webhookListener,
  id: 'selected-hook',
  name: 'Selected agent hook',
  assignments: {
    reviewer: { target_type: 'agent', agent_id: 'agent-1', conversation: 'current' },
  },
};

beforeEach(() => {
  invokeMock.mockReset();
  useListenersStore.setState({ listeners: [], gateway: null, loading: false, error: null });
});

describe('ListenersPanel', () => {
  it('creates a listener disabled, so a new watch never starts spending tokens on its own', async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === 'automation_list_blueprints') {
        return Promise.resolve({ blueprints: [{ id: 'audit', name: 'Audit', path: 'audit.md' }], truncated: false, next_offset: null });
      }
      if (command === 'listener_list') return Promise.resolve([]);
      if (command === 'listener_save') return Promise.resolve(webhookListener);
      if (command === 'listener_set_webhook_secret') return Promise.resolve('generated-secret');
      return Promise.resolve(null);
    });

    render(<ListenersPanel />);
    fireEvent.click(screen.getByRole('button', { name: /new listener/i }));

    await waitFor(() => expect(screen.getByLabelText('Automation')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: /^save$/i }));

    await waitFor(() => {
      const call = invokeMock.mock.calls.find(([command]) => command === 'listener_save');
      expect(call?.[1]).toMatchObject({ listener: { enabled: false } });
    });
  });

  it('shows a generated webhook secret exactly once and says it cannot be shown again', async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === 'automation_list_blueprints') {
        return Promise.resolve({ blueprints: [{ id: 'audit', name: 'Audit', path: 'audit.md' }], truncated: false, next_offset: null });
      }
      if (command === 'listener_list') return Promise.resolve([]);
      if (command === 'listener_save') return Promise.resolve(webhookListener);
      if (command === 'listener_set_webhook_secret') return Promise.resolve('generated-secret');
      return Promise.resolve(null);
    });

    render(<ListenersPanel />);
    fireEvent.click(screen.getByRole('button', { name: /new listener/i }));
    await waitFor(() => expect(screen.getByLabelText('Trigger')).toBeInTheDocument());
    fireEvent.change(screen.getByLabelText('Trigger'), { target: { value: 'webhook' } });
    fireEvent.click(screen.getByRole('button', { name: /^save$/i }));

    await waitFor(() => expect(screen.getByText('generated-secret')).toBeInTheDocument());
    expect(screen.getByText(/shown once/i)).toBeInTheDocument();
    expect(screen.getByText(/cannot show it again/i)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /dismiss/i }));
    expect(screen.queryByText('generated-secret')).not.toBeInTheDocument();
  });

  it('does not mint a secret for a listener that already has one', async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === 'automation_list_blueprints') {
        return Promise.resolve({ blueprints: [{ id: 'audit', name: 'Audit', path: 'audit.md' }], truncated: false, next_offset: null });
      }
      if (command === 'listener_list') return Promise.resolve([webhookListener]);
      if (command === 'listener_save') return Promise.resolve({ ...webhookListener, has_secret: true });
      return Promise.resolve(null);
    });

    useListenersStore.setState({ listeners: [webhookListener] });
    render(<ListenersPanel />);
    await waitFor(() => expect(screen.getByTestId('listener-row-saved-hook')).toBeInTheDocument());
    fireEvent.click(screen.getByLabelText('Edit CI hook'));
    fireEvent.click(screen.getByRole('button', { name: /^save$/i }));

    await waitFor(() =>
      expect(invokeMock.mock.calls.some(([command]) => command === 'listener_save')).toBe(true),
    );
    expect(
      invokeMock.mock.calls.some(([command]) => command === 'listener_set_webhook_secret'),
    ).toBe(false);
  });

  it('surfaces a save failure instead of closing the dialog on it', async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === 'automation_list_blueprints') {
        return Promise.resolve({ blueprints: [{ id: 'audit', name: 'Audit', path: 'audit.md' }], truncated: false, next_offset: null });
      }
      if (command === 'listener_list') return Promise.resolve([]);
      if (command === 'listener_save') {
        return Promise.reject(new Error('refusing to watch a path inside the Wardian home'));
      }
      return Promise.resolve(null);
    });

    render(<ListenersPanel />);
    fireEvent.click(screen.getByRole('button', { name: /new listener/i }));
    await waitFor(() => expect(screen.getByLabelText('Watch path')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: /^save$/i }));

    await waitFor(() =>
      expect(screen.getAllByText(/refusing to watch a path/i).length).toBeGreaterThan(0),
    );
    expect(screen.getByLabelText('Watch path')).toBeInTheDocument();
  });

  it('shows only listeners assigned to any selected agent', async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === 'automation_list_blueprints') return Promise.resolve({ blueprints: [], truncated: false, next_offset: null });
      if (command === 'listener_list') return Promise.resolve([webhookListener, assignedListener]);
      return Promise.resolve(null);
    });

    useListenersStore.setState({ listeners: [webhookListener, assignedListener] });
    render(<ListenersPanel selectedAgentIds={new Set(['agent-1', 'agent-2'])} />);

    await waitFor(() => expect(screen.getByTestId('listener-row-selected-hook')).toBeInTheDocument());
    expect(screen.queryByTestId('listener-row-saved-hook')).toBeNull();
    expect(screen.getByText('1 event listener for selected agents')).toBeInTheDocument();
  });
});
