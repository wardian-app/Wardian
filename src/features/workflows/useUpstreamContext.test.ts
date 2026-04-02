import { describe, expect, it } from 'vitest';
import { flattenKeys, getDeepKeys } from './useUpstreamContext';

describe('useUpstreamContext helpers', () => {
  it('extracts built-in fields for command, loop, and trigger nodes', () => {
    const commandKeys = getDeepKeys({ type: 'command', data: { config: {} } }).map((k) => k.path);
    const loopKeys = getDeepKeys({ type: 'loop', data: { config: { iterator_name: 'item' } } }).map((k) => k.path);
    const fileWatcherKeys = getDeepKeys({
      type: 'trigger',
      data: { blockName: 'File Watcher', config: { custom: 'v' } },
    }).map((k) => k.path);
    const scheduledKeys = getDeepKeys({
      type: 'trigger',
      data: { blockName: 'Scheduled Trigger', config: {} },
    }).map((k) => k.path);

    expect(commandKeys).toEqual(expect.arrayContaining(['stdout', 'stderr', 'exit_code']));
    expect(loopKeys).toContain('item');
    expect(fileWatcherKeys).toEqual(expect.arrayContaining(['path', 'event', 'timestamp', 'custom']));
    expect(scheduledKeys).toEqual(expect.arrayContaining(['timestamp', 'id']));
  });

  it('expands json_schema properties recursively and skips schema config keys', () => {
    const node = {
      type: 'agent',
      data: {
        config: {
          json_schema: JSON.stringify({
            type: 'object',
            properties: {
              summary: { type: 'string' },
              details: {
                type: 'object',
                properties: {
                  score: { type: 'number' },
                },
              },
            },
          }),
        },
      },
    };

    const keys = getDeepKeys(node);
    const flatPaths = flattenKeys(keys).map((k) => k.path);

    expect(flatPaths).toEqual(expect.arrayContaining(['summary', 'details', 'details.score']));
    expect(flatPaths).not.toContain('json_schema');
  });

  it('supports input_schema sample objects and ignores top-level schema markers', () => {
    const node = {
      type: 'tool',
      data: {
        config: {
          input_schema: JSON.stringify({
            type: 'object',
            required: ['name'],
            name: 'alice',
            meta: { age: 5 },
          }),
        },
      },
    };

    const flatPaths = flattenKeys(getDeepKeys(node)).map((k) => k.path);
    expect(flatPaths).toEqual(expect.arrayContaining(['name', 'meta', 'meta.age']));
    expect(flatPaths).not.toEqual(expect.arrayContaining(['type', 'properties', 'required']));
  });

  it('returns built-in keys when schema parsing fails', () => {
    const node = {
      type: 'command',
      data: { config: { json_schema: '{not-valid-json' } },
    };

    const paths = getDeepKeys(node).map((k) => k.path);
    expect(paths).toEqual(expect.arrayContaining(['stdout', 'stderr', 'exit_code']));
  });

  it('handles empty input and flattenKeys recursion', () => {
    expect(getDeepKeys(null)).toEqual([]);

    const nested = [
      { path: 'a', label: 'a', children: [{ path: 'a.b', label: 'b', children: [{ path: 'a.b.c', label: 'c' }] }] },
    ];
    expect(flattenKeys(nested as any).map((k) => k.path)).toEqual(['a', 'a.b', 'a.b.c']);
  });
});
