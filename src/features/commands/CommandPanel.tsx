import React from "react";

interface CommandPanelProps {
  selectedAgentCount: number;
  broadcastMessage: string;
  setBroadcastMessage: (msg: string) => void;
  onBroadcast: (e: React.FormEvent) => void;
  onCollapse: () => void;
}

export const CommandPanel: React.FC<CommandPanelProps> = ({
  selectedAgentCount,
  broadcastMessage,
  setBroadcastMessage,
  onBroadcast,
  onCollapse,
}) => {
  return (
    <div className="flex flex-col h-full">
      <div className="flex justify-between items-center mb-6">
        <h2 className="text-xl font-bold text-primary tracking-tight">COMMAND</h2>
        <button onClick={onCollapse} className="text-bright-neutral hover:text-primary transition-colors">
          <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M15 19l-7-7 7-7"></path></svg>
        </button>
      </div>

      <div className="mb-8">
        <h3 className="text-xs font-bold text-muted uppercase tracking-widest mb-4">Quick Actions</h3>
        <div className="grid grid-cols-1 gap-2">
           <button className="flex items-center gap-3 p-3 bg-wardian-card-bg-muted border border-wardian-light/50 rounded-lg text-muted-neutral hover:text-[var(--color-wardian-accent)] hover:border-[var(--color-wardian-accent)]/30 transition-all text-left group">
              <svg className="w-4 h-4 opacity-50 group-hover:opacity-100" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"></path></svg>
              <span className="text-xs font-bold">Summarize Day</span>
           </button>
           <button className="flex items-center gap-3 p-3 bg-wardian-card-bg-muted border border-wardian-light/50 rounded-lg text-muted-neutral hover:text-[var(--color-wardian-accent)] hover:border-[var(--color-wardian-accent)]/30 transition-all text-left group">
              <svg className="w-4 h-4 opacity-50 group-hover:opacity-100" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2m-6 9l2 2 4-4"></path></svg>
              <span className="text-xs font-bold">Run Health Check</span>
           </button>
        </div>
      </div>

      <div className="mt-auto pt-6 border-t border-wardian-border">
        <h3 className="text-xs font-bold text-muted uppercase tracking-widest mb-4">Broadcast</h3>
        <form onSubmit={onBroadcast} className="flex flex-col gap-2">
          <textarea
            className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-primary text-xs focus:outline-none focus:border-[var(--color-wardian-accent)] h-32 resize-none"
            placeholder={selectedAgentCount > 0 ? `Message ${selectedAgentCount} selected...` : "Broadcast to all agents..."}
            value={broadcastMessage}
            onChange={(e) => setBroadcastMessage(e.currentTarget.value)}
          />
          <button
            type="submit"
            className="bg-wardian-success/20 hover:bg-wardian-success/40 border border-wardian-success/30 text-wardian-success font-bold py-2 rounded text-[10px] uppercase tracking-wider transition-colors"
          >
            Execute Broadcast
          </button>
        </form>
      </div>
    </div>
  );
};
