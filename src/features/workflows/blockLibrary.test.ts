import { describe, expect, it } from 'vitest';
import { BLOCK_LIBRARY } from './blockLibrary';

describe('workflow block library', () => {
  it('exposes exactly the three supported agent run modes', () => {
    const agentBlock = BLOCK_LIBRARY.find((block) => block.type === 'agent');
    const modeField = agentBlock?.fields?.find((field) => field.name === 'mode');

    expect(modeField?.options).toEqual(['ephemeral', 'inherit_fresh', 'inherit_resume']);
  });

  it('does not expose legacy session_type or session_persistence fields for agent nodes', () => {
    const agentBlock = BLOCK_LIBRARY.find((block) => block.type === 'agent');
    const fieldNames = [
      ...(agentBlock?.fields || []),
      ...(agentBlock?.advancedFields || []),
    ].map((field) => field.name);

    expect(fieldNames).not.toContain('session_type');
    expect(fieldNames).not.toContain('session_persistence');
  });
});
