import React, { useState } from 'react';

// Icons
const StopIcon = () => <svg className="w-3 h-3" fill="currentColor" viewBox="0 0 20 20"><path d="M5 5h10v10H5z" /></svg>;
const ActivityIcon = () => <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth="2"><path d="M13 10V3L4 14h7v7l9-11h-7z" /></svg>;

interface ActiveMonitoringProps {
  activeRuns: any[];
  schedules: any[];
  activeWorkflows: any[];
  onStopRun: (id: string) => void;
  onStopTrigger: (id: string) => void;
  onToggleSchedule: (id: string) => void;
}

export const ActiveMonitoring: React.FC<ActiveMonitoringProps> = ({
  activeRuns,
  schedules,
  activeWorkflows,
  onStopRun,
  onStopTrigger,
  onToggleSchedule
}) => {
  const [openSections, setOpenSections] = useState({
    runs: true,
    triggers: true,
    schedules: true
  });

  const toggle = (section: keyof typeof openSections) => 
    setOpenSections(prev => ({ ...prev, [section]: !prev[section] }));

  const SectionHeader = ({ title, count, section }: { title: string, count: number, section: keyof typeof openSections }) => (
    <button 
      onClick={() => toggle(section)}
      className="w-full flex items-center justify-between py-2 text-xs font-bold text-muted-neutral hover:text-primary transition-colors tracking-wide border-t border-wardian-border/10 first:border-t-0 mt-1"
    >
      <div className="flex items-center gap-2">
        <span>{title}</span>
        {count > 0 && <span className="text-[9px] bg-white/5 px-1.5 py-0.5 rounded-full font-mono">{count}</span>}
      </div>
      <span className={`transform transition-transform duration-200 opacity-30 ${openSections[section] ? 'rotate-0' : '-rotate-90'}`}>▼</span>
    </button>
  );

  return (
    <div className="flex flex-col gap-1">
      {/* 2. ACTIVE RUNS */}
      <SectionHeader title="Active Runs" count={activeRuns.length} section="runs" />
      {openSections.runs && (
        <div className="px-2 space-y-2 pb-3 pt-1">
          {activeRuns.length === 0 ? (
            <div className="text-[9px] text-muted-neutral italic py-4 text-center border border-dashed border-wardian-border/20 rounded-lg">No active simulations</div>
          ) : (
            activeRuns.map(run => (
              <div key={run.run_id} className="p-2.5 rounded-lg bg-[var(--color-wardian-card-bg)] border border-wardian-border/40 group hover:border-[var(--color-wardian-accent)]/30 transition-colors">
                <div className="flex justify-between items-start mb-2">
                  <span className="text-[11px] font-bold text-primary truncate leading-tight tracking-tight">{run.workflow_name}</span>
                  <button 
                    onClick={() => onStopRun(run.run_id)}
                    className="opacity-0 group-hover:opacity-100 p-1 text-muted hover:text-red-500 transition-all hover:scale-110"
                    title="Terminate Run"
                  >
                    <StopIcon />
                  </button>
                </div>
                <div className="flex items-center justify-between text-[9px] mb-1.5">
                  <span className="text-cyan-400 font-bold tracking-wide">Step {run.current_step}/{run.total_steps}</span>
                  <span className="text-muted-neutral truncate italic max-w-[60%]">{run.active_node_name}</span>
                </div>
                <div className="w-full bg-[var(--color-wardian-input-bg)] h-1.5 rounded-full overflow-hidden border border-white/5">
                  <div 
                    className="bg-cyan-500 h-full transition-all duration-700 cubic-bezier(0.4, 0, 0.2, 1)" 
                    style={{ width: `${(run.current_step / run.total_steps) * 100}%` }} 
                  />
                </div>
              </div>
            ))
          )}
        </div>
      )}

      {/* 3. ACTIVE LISTENERS / TRIGGERS */}
      <SectionHeader title="Live Listeners" count={activeWorkflows.length} section="triggers" />
      {openSections.triggers && (
        <div className="px-2 space-y-1 pb-3 pt-1">
          {activeWorkflows.length === 0 ? (
            <div className="text-[9px] text-muted-neutral italic py-4 text-center border border-dashed border-wardian-border/20 rounded-lg">Default awareness active</div>
          ) : (
            activeWorkflows.map(wf => (
              <div key={wf.id} className="flex justify-between items-center px-3 py-2 rounded-lg bg-emerald-500/5 border border-emerald-500/10 group hover:bg-emerald-500/10 transition-colors">
                <div className="flex items-center gap-2.5 truncate">
                  <div className="relative">
                    <div className="w-2 h-2 rounded-full bg-emerald-400" />
                  </div>
                  <span className="text-[11px] font-semibold text-emerald-400 truncate tracking-tight">{wf.name}</span>
                </div>
                <button 
                  onClick={() => onStopTrigger(wf.id)}
                  className="opacity-0 group-hover:opacity-100 p-1 text-emerald-600 hover:text-red-500 transition-all"
                  title="Pause Listener"
                >
                  <StopIcon />
                </button>
              </div>
            ))
          )}
        </div>
      )}

      {/* 4. SCHEDULES */}
      <SectionHeader title="Scheduled Tasks" count={schedules.length} section="schedules" />
      {openSections.schedules && (
        <div className="px-2 space-y-1 pb-4 pt-1">
          {schedules.length === 0 ? (
            <div className="text-[9px] text-muted-neutral italic py-4 text-center border border-dashed border-wardian-border/20 rounded-lg">No future events sync'd</div>
          ) : (
            schedules.map(s => (
              <div key={s.id} className="flex justify-between items-center px-3 py-2 rounded-lg bg-[var(--color-wardian-input-bg)] border border-wardian-border/20 group hover:border-white/10 transition-colors">
                <div className="flex flex-col truncate pr-3">
                  <span className="text-[11px] font-bold text-primary truncate leading-tight tracking-tight">{s.workflow_name}</span>
                  <div className="flex items-center gap-2 mt-0.5">
                    <span className="text-[9px] font-mono text-muted-neutral tracking-tight">{s.frequency}</span>
                    <span className="text-[8px] text-[var(--color-wardian-accent)] opacity-60 font-bold">• NEXT IN 42M</span>
                  </div>
                </div>
                <div className="flex items-center gap-2.5">
                  <span className={`text-[8px] font-black px-1.5 py-0.5 rounded ${s.is_paused ? 'bg-amber-500/20 text-amber-500' : 'bg-emerald-500/20 text-emerald-400'}`}>
                    {s.is_paused ? 'Paused' : 'Live'}
                  </span>
                  <button 
                    onClick={() => onToggleSchedule(s.id)}
                    className="opacity-0 group-hover:opacity-100 p-1 text-muted hover:text-primary transition-all"
                  >
                    {s.is_paused ? <ActivityIcon /> : <StopIcon />}
                  </button>
                </div>
              </div>
            ))
          )}
        </div>
      )}
    </div>
  );
};
