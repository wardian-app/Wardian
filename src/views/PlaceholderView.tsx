import React from "react";

interface PlaceholderViewProps {
  viewMode: string;
}

export const PlaceholderView: React.FC<PlaceholderViewProps> = ({ viewMode }) => {
  const phase = (viewMode === 'queue' || viewMode === 'workflow-builder') ? '3' : '5';
  
  return (
    <div className="flex-1 flex flex-col items-center justify-center text-center p-12 border border-gray-800/50 rounded-2xl bg-gray-900/20 backdrop-blur-sm animate-in fade-in zoom-in duration-500 placeholder-view">
      <div className="w-24 h-24 mb-6 text-gray-800/30 placeholder-icon-container animate-float block">
        {viewMode === "queue" && <svg fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.5" d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2m-6 9l2 2 4-4"></path></svg>}
        {viewMode === "workflow-builder" && <svg fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.5" d="M9 17V7m0 10a2 2 0 01-2 2H5a2 2 0 01-2-2V7a2 2 0 012-2h2a2 2 0 012 2m0 10a2 2 0 002 2h2a2 2 0 002-2V7a2 2 0 00-2-2h-2a2 2 0 00-2 2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z"></path></svg>}
        {viewMode === "graph" && <svg fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.5" d="M13 10V3L4 14h7v7l9-11h-7z"></path></svg>}
        {viewMode === "garden" && <svg fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.5" d="M12 6V4m0 2a2 2 0 100 4m0-4a2 2 0 110 4m-6 8a2 2 0 100-4m0 4a2 2 0 110-4m0 4v2m0-6V4m6 6v10m6-2a2 2 0 100-4m0 4a2 2 0 110-4m0 4v2m0-6V4"></path></svg>}
      </div>
      <h2 className="text-2xl font-bold text-primary mb-2 tracking-tight">
        {viewMode.split("-").map(word => word.charAt(0).toUpperCase() + word.slice(1)).join(" ")}
      </h2>
      <p className="text-muted max-w-md mx-auto mb-8 font-medium italic">
        Advanced {viewMode === 'queue' ? 'human-in-the-loop' : viewMode} features coming in Phase {phase}.
      </p>
      <div className="flex gap-4">
        <div className="px-6 py-2 rounded-full border border-gray-800 text-[10px] font-bold text-muted-neutral tracking-wide">Planned</div>
        <div className="px-6 py-2 rounded-full bg-[var(--color-wardian-accent)]/10 text-[10px] font-bold text-[var(--color-wardian-accent)] tracking-wide border border-[var(--color-wardian-accent)]/20">Phase {phase}</div>
      </div>
    </div>
  );
};
