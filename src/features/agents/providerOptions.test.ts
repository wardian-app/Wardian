import { describe, expect, it } from 'vitest';
import type { ProviderReadiness } from '../../types';
import { buildProviderOptions, buildUngatedProviderOptions, resolveEffectiveProvider } from './providerOptions';

const readiness = (provider: ProviderReadiness['provider'], available: boolean): ProviderReadiness => ({
  provider,
  display_name: provider === 'opencode' ? 'OpenCode' : provider[0].toUpperCase() + provider.slice(1),
  available,
  executable: available ? provider : null,
  reason: available ? null : `${provider} missing`,
});

describe('provider option helpers', () => {
  it('builds enabled fallback options when readiness is unknown', () => {
    expect(buildUngatedProviderOptions()).toEqual([
      { value: 'claude', label: 'Claude', available: true, reason: null },
      { value: 'codex', label: 'Codex', available: true, reason: null },
      { value: 'antigravity', label: 'Antigravity', available: true, reason: null },
      { value: 'opencode', label: 'OpenCode', available: true, reason: null },
      { value: 'prime', label: 'Prime Agent', available: true, reason: null },
      { value: 'gemini', label: 'Gemini', available: true, reason: null },
    ]);
  });

  it('labels Prime Agent and surfaces its readiness reason', () => {
    expect(buildProviderOptions([readiness('prime', true)]).find((option) => option.value === 'prime')).toEqual({
      value: 'prime',
      label: 'Prime Agent',
      available: true,
      reason: null,
    });

    const missing = buildProviderOptions([
      {
        provider: 'prime',
        display_name: 'Prime Agent',
        available: false,
        executable: null,
        reason: 'Prime Agent is installed but its Python kernel is not set up.',
      },
    ]).find((option) => option.value === 'prime');

    expect(missing?.label).toBe('Prime Agent - not installed');
    expect(missing?.reason).toBe('Prime Agent is installed but its Python kernel is not set up.');
  });

  it('does not call an installed provider "not installed" when its runtime is the blocker', () => {
    const blocked = buildProviderOptions([
      {
        provider: 'prime',
        display_name: 'Prime Agent',
        available: false,
        executable: 'C:/npm/prime-agent',
        reason: 'Prime Agent is installed but its Python kernel is not set up.',
      },
    ]).find((option) => option.value === 'prime');

    expect(blocked?.label).toBe('Prime Agent - needs setup');
    expect(blocked?.available).toBe(false);
  });

  it('explains a runtime blocker in the fallback note instead of blaming the install', () => {
    const result = resolveEffectiveProvider(
      [
        readiness('claude', true),
        {
          provider: 'prime',
          display_name: 'Prime Agent',
          available: false,
          executable: 'C:/npm/prime-agent',
          reason: 'Prime Agent is installed but its Python kernel is not set up.',
        },
      ],
      'prime',
    );

    expect(result.provider).toBe('claude');
    expect(result.note).toBe(
      'Default provider Prime Agent needs setup. Using Claude. Prime Agent is installed but its Python kernel is not set up.',
    );
  });

  it('does not expose maintenance status in user-facing provider labels', () => {
    expect(buildUngatedProviderOptions().find((option) => option.value === 'gemini')?.label).toBe('Gemini');
    expect(buildProviderOptions([readiness('gemini', false)]).find((option) => option.value === 'gemini')?.label).toBe(
      'Gemini - not installed',
    );
  });

  it('uses canonical labels for known providers when readiness labels are stale', () => {
    expect(buildProviderOptions([
      {
        provider: 'antigravity',
        display_name: 'antigravity',
        available: true,
        executable: 'agy',
        reason: null,
      },
    ])).toContainEqual({
      value: 'antigravity',
      label: 'Antigravity',
      available: true,
      reason: null,
    });
  });

  it('auto prefers Claude when available', () => {
    const result = resolveEffectiveProvider([
      readiness('claude', true),
      readiness('codex', true),
    ], 'auto');

    expect(result).toEqual({ provider: 'claude', note: null });
  });

  it('auto falls back to the first available provider when Claude is unavailable', () => {
    const result = resolveEffectiveProvider([
      readiness('claude', false),
      readiness('codex', true),
      readiness('gemini', true),
    ], 'auto');

    expect(result).toEqual({ provider: 'codex', note: null });
  });

  it('uses an explicit available default provider', () => {
    const result = resolveEffectiveProvider([
      readiness('claude', true),
      readiness('codex', true),
    ], 'codex');

    expect(result).toEqual({ provider: 'codex', note: null });
  });

  it('falls back with a note when the explicit default provider is unavailable', () => {
    const result = resolveEffectiveProvider([
      readiness('claude', true),
      readiness('codex', false),
    ], 'codex');

    expect(result).toEqual({
      provider: 'claude',
      note: 'Default provider Codex is not installed. Using Claude. codex missing',
    });
  });

  it('returns null when no providers are available', () => {
    const result = resolveEffectiveProvider([
      readiness('claude', false),
      readiness('codex', false),
    ], 'auto');

    expect(result).toEqual({ provider: null, note: 'No supported provider CLI was found.' });
  });
});
