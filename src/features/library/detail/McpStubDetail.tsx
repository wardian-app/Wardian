import React from 'react';

/**
 * Detail-pane counterpart to LibraryList's MCP stub: the MCP section has no
 * entries yet (`stubbed: true`, empty tree), so DetailPane shows this in
 * place of the generic empty state whenever the MCP section is active.
 */
export const McpStubDetail: React.FC = () => (
    <div data-testid="mcp-stub-detail" className="h-full flex flex-col items-center justify-center gap-2 px-6 text-center">
        <h3 className="text-sm font-medium text-primary">MCP servers are coming to the library</h3>
        <p className="text-xs text-muted">
            Define once, deploy to agents and classes — the same scoping skills use today.
        </p>
    </div>
);
