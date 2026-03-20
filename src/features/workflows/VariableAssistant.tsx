import { memo, useState } from 'react';
import { BLOCK_LIBRARY } from './blockLibrary';
import { useUpstreamContext, getDeepKeys } from './useUpstreamContext';

interface VariableAssistantProps {
  selectedNodeId: string;
}

export const VariableAssistant = memo(({ selectedNodeId }: VariableAssistantProps) => {
  const [search, setSearch] = useState("");
  const [copiedPath, setCopiedPath] = useState<string | null>(null);

  const upstreamContext = useUpstreamContext(selectedNodeId);

  const copyVariable = (path: string) => {
    const tag = `{{${path}}}`;
    navigator.clipboard.writeText(tag);
    setCopiedPath(path);
    setTimeout(() => setCopiedPath(null), 1500);
  };


  const renderVariableTree = (keys: any[], baseId: string, accentColor: string = 'var(--color-wardian-accent)', level: number = 0) => {
    return keys.map((key: any) => (
      <div key={key.path} className="space-y-0.5">
        <div 
          onClick={(e) => { e.stopPropagation(); copyVariable(`${baseId}.${key.path}`); }}
          className={`${level > 0 ? 'ml-4' : ''} group flex items-center justify-between p-1.5 rounded-md bg-white/5 border border-transparent hover:border-wardian-border transition-all cursor-pointer`}
        >
          <div className="flex flex-col">
            <span className="text-[10px] font-bold text-[var(--color-wardian-text)] group-hover:text-[var(--color-wardian-accent)] transition-colors">
              {key.label}
            </span>
            <span className="text-[8px] font-mono text-[var(--color-wardian-text-muted)] opacity-50">
              {baseId}.{key.path}
            </span>
          </div>
          {copiedPath === `${baseId}.${key.path}` ? (
            <span className={`text-[8px] font-bold animate-in fade-in slide-in-from-right-1 duration-200 tracking-wide`} style={{ color: accentColor }}>Copied</span>
          ) : (
            <svg className="w-3.5 h-3.5 opacity-40 group-hover:opacity-100 transition-all flex-shrink-0" style={{ color: accentColor }} fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M8 5H6a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2v-1M8 5a2 2 0 002 2h2a2 2 0 002-2M8 5a2 2 0 012-2h2a2 2 0 012 2m0 0h2a2 2 0 012 2v3m2 4H10m0 0l3-3m-3 3l3 3"></path>
            </svg>
          )}
        </div>
        {key.children && renderVariableTree(key.children, baseId, accentColor, level + 1)}
      </div>
    ));
  };

  const renderNodeTree = (node: any, isPrevious: boolean = false) => {
    const blockDef = BLOCK_LIBRARY.find(b => b.type === node.type && (node.data?.blockName ? b.name === node.data.blockName : true));
    const blockName = blockDef?.name || node.type?.charAt(0).toUpperCase() + node.type?.slice(1).toLowerCase();
    const label = node.data?.label || node.id;
    const baseId = `nodes.${node.id}.output`;
    const deepKeys = getDeepKeys(node);

    return (
      <div key={node.id} className="space-y-1">
        <div 
          onClick={() => copyVariable(baseId)}
          className="group flex items-center justify-between p-2 rounded-lg bg-[var(--color-wardian-bg)] border border-wardian-border hover:border-[var(--color-wardian-accent)] transition-all cursor-pointer"
        >
          <div className="flex flex-col">
            <span className="text-[10px] font-bold text-[var(--color-wardian-text)] group-hover:text-[var(--color-wardian-accent)] transition-colors">
              {label} <span className="text-muted-neutral font-normal opacity-50 mx-1">|</span> Output
            </span>
            <span className="text-[8px] font-medium text-[var(--color-wardian-text-muted)] opacity-70">
              {blockName} {isPrevious && <span className="text-[var(--color-wardian-warning)] font-bold italic ml-1 tracking-tight">(Prev Iter)</span>}
            </span>
            <span className="text-[8px] font-mono text-[var(--color-wardian-text-muted)] opacity-70 mt-0.5">
              {baseId}
            </span>
          </div>
          {copiedPath === baseId ? (
            <span className={`text-[9px] font-bold ${isPrevious ? 'text-[var(--color-wardian-warning)]' : 'text-[var(--color-wardian-accent)]'} animate-in fade-in slide-in-from-right-1 duration-200 tracking-wide`}>Copied</span>
          ) : (
            <svg className="w-3.5 h-3.5 text-[var(--color-wardian-text-muted)] group-hover:text-[var(--color-wardian-accent)] opacity-40 group-hover:opacity-100 transition-all flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M8 5H6a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2v-1M8 5a2 2 0 002 2h2a2 2 0 002-2M8 5a2 2 0 012-2h2a2 2 0 012 2m0 0h2a2 2 0 012 2v3m2 4H10m0 0l3-3m-3 3l3 3"></path>
            </svg>
          )}
        </div>

        {/* Recursive Tree */}
        <div className="space-y-0.5 mt-1">
          {renderVariableTree(deepKeys, baseId)}
        </div>
      </div>
    );
  };

  return (
    <div 
      onContextMenu={(e) => e.preventDefault()}
      className="flex flex-col h-full bg-white/5 border border-wardian-border rounded-xl overflow-hidden shadow-2xl"
    >
      <div className="p-4 border-b border-wardian-border bg-white/5">
        <div className="flex items-center justify-between mb-3">
          <span className="text-[10px] font-black text-muted-neutral tracking-wide">Variable Assistant</span>
        </div>
        <input 
          placeholder="Filter context..."
          className="w-full bg-[var(--color-wardian-bg)] border border-wardian-border rounded-lg px-3 py-1.5 text-[10px] text-primary outline-none focus:ring-1 focus:ring-[var(--color-wardian-accent)] transition-all"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-6 no-scrollbar min-h-0">

        <section className="space-y-2">
          <h4 className="text-[9px] font-bold text-muted-neutral tracking-wide px-1">Upstream Outputs</h4>
          <div className="space-y-1.5">
            {upstreamContext.currentNodes.length === 0 && (
              <div className="text-[9px] text-muted-neutral italic p-2 opacity-50">No upstream nodes found</div>
            )}
            {upstreamContext.currentNodes.filter((n: any) => !search || String(n.data?.label || '').toLowerCase().includes(search.toLowerCase()) || String(n.id || '').toLowerCase().includes(search.toLowerCase())).map((n: any) => renderNodeTree(n))}
          </div>
        </section>

        {upstreamContext.previousNodes.length > 0 && (
          <section className="space-y-2">
            <h4 className="text-[9px] font-bold text-[var(--color-wardian-warning)] tracking-wide px-1 opacity-80">Cycle Contexts</h4>
            <div className="space-y-1.5">
              {upstreamContext.previousNodes.filter((n: any) => !search || String(n.data?.label || '').toLowerCase().includes(search.toLowerCase()) || String(n.id || '').toLowerCase().includes(search.toLowerCase())).map((n: any) => renderNodeTree(n, true))}
            </div>
          </section>
        )}

        <section className="space-y-2 pt-4 border-t border-white/5">
          <h4 className="text-[9px] font-bold text-muted-neutral tracking-wide px-1">Shared Storage</h4>
          <div 
            onClick={() => copyVariable(`storage.key`)}
            className="group flex flex-col p-2 rounded-lg bg-[var(--color-wardian-bg)] border border-wardian-border hover:border-[var(--color-wardian-accent)] transition-all cursor-pointer"
          >
            <div className="flex items-center justify-between">
              <div className="flex flex-col">
                <span className="text-[10px] font-bold text-[var(--color-wardian-text)]">
                  KV Store <span className="text-muted-neutral font-normal opacity-30 mx-1">|</span> Value
                </span>
                <span className="text-[8px] font-medium text-[var(--color-wardian-text-muted)] opacity-70">Persistent Store</span>
              </div>
              {copiedPath === `storage.key` ? (
                <span className="text-[9px] font-bold text-[var(--color-wardian-accent)] animate-in fade-in slide-in-from-right-1 duration-200">COPIED</span>
              ) : (
                <svg className="w-3.5 h-3.5 text-[var(--color-wardian-accent)] opacity-40 group-hover:opacity-100 transition-all flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M8 5H6a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2v-1M8 5a2 2 0 002 2h2a2 2 0 002-2M8 5a2 2 0 012-2h2a2 2 0 012 2m0 0h2a2 2 0 012 2v3m2 4H10m0 0l3-3m-3 3l3 3"></path>
                </svg>
              )}
            </div>
            <span className="text-[8px] font-mono text-[var(--color-wardian-text-muted)] opacity-70 mt-0.5">storage.*</span>
          </div>
        </section>
      </div>
    </div>
  );
});
