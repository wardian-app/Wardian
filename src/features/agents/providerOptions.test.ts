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
      { value: 'gemini', label: 'Gemini', available: true, reason: null },
    ]);
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
      note: 'Default provider Codex is not installed. Using Claude.',
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
