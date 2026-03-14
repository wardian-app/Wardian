import React, { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { 
  WorkflowSummary, 
  ScheduledRun, 
  ActiveRunTracker,
  WorkflowTriggerStatus
} from '../../types';

// Icons (using basic SVGs aligned with Wardian style)
const PlayIcon = () => <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 20 20"><path d="M4 4l12 6-12 6z" /></svg>;
const PauseIcon = () => <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 20 20"><path d="M5 4h3v12H5zm7 0h3v12h-3z" /></svg>;
const StopIcon = () => <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 20 20"><path d="M5 5h10v10H5z" /></svg>;
const EditIcon = () => <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 20 20"><path d="M17.414 2.586a2 2 0 00-2.828 0L7 10.172V13h2.828l7.586-7.586a2 2 0 000-2.828z" /><path d="M2 6a2 2 0 012-2h4a1 1 0 010 2H4v10h10v-4a1 1 0 112 0v4a2 2 0 01-2 2H4a2 2 0 01-2-2V6z" /></svg>;
const TrashIcon = () => <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 20 20"><path d="M9 2a1 1 0 00-.894.553L7.382 4H4a1 1 0 000 2v10a2 2 0 002 2h8a2 2 0 002-2V6a1 1 0 100-2h-3.382l-.724-1.447A1 1 0 0011 2H9zM7 8a1 1 0 012 0v6a1 1 0 11-2 0V8zm5-1a1 1 0 00-1 1v6a1 1 0 102 0V8a1 1 0 00-1-1z" /></svg>;
const EyeIcon = () => <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 20 20"><path d="M10 12a2 2 0 100-4 2 2 0 000 4z" /><path d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" /></svg>;
const PowerIcon = () => <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 20 20"><path d="M10 2a1 1 0 011 1v7a1 1 0 11-2 0V3a1 1 0 011-1zm4.586 2.586a1 1 0 00-1.414 1.414 7 7 0 11-6.344 0 1 1 0 10-1.414-1.414 9 9 0 109.172 0z" /></svg>;
const PlusIcon = () => <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 20 20"><path d="M10 3a1 1 0 011 1v5h5a1 1 0 110 2h-5v5a1 1 0 11-2 0v-5H4a1 1 0 110-2h5V4a1 1 0 011-1z" /></svg>;

const ClockIcon = () => <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" strokeWidth="2"/><path d="M12 6v6l4 2" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"/></svg>;
const LinkIcon = () => <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path d="M10 13a5 5 0 007.54.54l3-3a5 5 0 00-7.07-7.07l-1.72 1.71" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"/><path d="M14 11a5 5 0 00-7.54-.54l-3 3a5 5 0 007.07 7.07l1.71-1.71" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"/></svg>;
const FolderIcon = () => <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" /></svg>;


// MOCK DATA for Phase 1
const mockSchedules: ScheduledRun[] = [
  { id: 's1', workflow_id: 'w1', workflow_name: 'Daily Backup', next_run_epoch_ms: Date.now() + 3600000, frequency: 'Daily at 2 AM', is_paused: false },
  { id: 's2', workflow_id: 'wx', workflow_name: 'Weekly Digest', next_run_epoch_ms: Date.now() + 86400000, frequency: 'Every Sunday', is_paused: true },
];

const mockActiveRuns: ActiveRunTracker[] = [
  { run_id: 'r1', workflow_id: 'w2', workflow_name: 'File Watcher Sync', current_step: 3, total_steps: 8, active_node_name: 'Copy Files' }
];

interface WorkflowSidebarProps {
  onCollapse: () => void;
}

export const WorkflowSidebar: React.FC<WorkflowSidebarProps> = ({ onCollapse }) => {
  const [workflows, setWorkflows] = useState<WorkflowSummary[]>([]);
  const [schedules] = useState<ScheduledRun[]>(mockSchedules);
  const [activeRuns] = useState<ActiveRunTracker[]>(mockActiveRuns);

  const fetchWorkflows = async () => {
    try {
      const list = await invoke<any[]>('list_workflows');
      // Map backend WorkflowDefinition to WorkflowSummary
      const summaries: WorkflowSummary[] = list.map(wf => ({
        id: wf.id,
        name: wf.name,
        trigger_type: 'manual', // Default for now until we parse trigger nodes
        trigger_status: 'off',
      }));
      setWorkflows(summaries);
    } catch (error) {
      console.error('Failed to fetch workflows', error);
    }
  };

  React.useEffect(() => {
    fetchWorkflows();
  }, []);
  
  const [sectionOpen, setSectionOpen] = useState({
    workflows: true,
    schedules: true,
    activeRuns: true
  });

  const toggleSection = (section: keyof typeof sectionOpen) => {
    setSectionOpen(prev => ({ ...prev, [section]: !prev[section] }));
  };

  const handleMuteAll = async () => {
    if (!confirm('PANIC: This will physically detach all active triggers. Proceed?')) return;
    try {
      await invoke('mute_all_triggers');
      fetchWorkflows(); // Refresh UI
    } catch (error) {
      console.error('Failed to mute all triggers', error);
    }
  };

  const getStatusColor = (status: WorkflowTriggerStatus) => {
    switch (status) {
      case 'active': return 'bg-emerald-500';
      case 'muted': return 'bg-amber-500';
      case 'off': return 'bg-gray-500';
      default: return 'bg-gray-500';
    }
  };

  const getTriggerIcon = (type: string) => {
    switch (type) {
      case 'cron': return <ClockIcon />;
      case 'webhook': return <LinkIcon />;
      case 'watcher': return <FolderIcon />;
      default: return <span className="w-4 h-4 inline-block text-xs font-bold text-center leading-4">M</span>;
    }
  };

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="flex justify-between items-center mb-4 pb-2 border-b border-wardian-border">
        <h2 className="text-xl font-bold text-primary tracking-tight">AUTOMATION</h2>
        <button onClick={onCollapse} className="text-bright-neutral hover:text-primary transition-colors">
          <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M15 19l-7-7 7-7" />
          </svg>
        </button>
      </div>

      {/* Global Controls */}
      <div className="flex flex-col gap-2 mb-6">
        <input 
          type="text" 
          placeholder="Search workflows..." 
          className="w-full bg-[var(--color-wardian-surface)] border border-wardian-border text-primary text-sm rounded px-3 py-1.5 focus:outline-none focus:border-cyan-500 transition-colors"
        />
        <div className="flex gap-2">
          <button 
            onClick={handleMuteAll}
            className="flex-1 flex items-center justify-center gap-2 bg-red-500/10 hover:bg-red-500/20 text-red-400 text-[11px] font-bold px-3 py-2 rounded-lg border border-red-500/30 transition-all hover:shadow-[0_0_10px_rgba(239,68,68,0.2)] active:scale-95"
            title="Instant global silencing of all autonomous entry points"
          >
            <PowerIcon /> PANIC
          </button>
          <button 
            className="flex-1 flex items-center justify-center gap-2 bg-emerald-500/10 hover:bg-emerald-500/20 text-emerald-400 text-[11px] font-bold px-3 py-2 rounded-lg border border-emerald-500/30 transition-all hover:shadow-[0_0_10px_rgba(16,185,129,0.2)] active:scale-95"
          >
            <PlusIcon /> CREATE
          </button>
        </div>
      </div>

      <div className="flex-1 overflow-y-auto no-scrollbar pr-1 -mr-1 space-y-4">
        
        {/* Active Runs Section */}
        <div className="border border-wardian-border rounded-lg bg-[var(--color-wardian-surface)] overflow-hidden">
          <button 
            onClick={() => toggleSection('activeRuns')}
            className="w-full flex items-center justify-between px-3 py-2 bg-[var(--color-wardian-surface-hover)] text-sm font-semibold text-primary transition-colors hover:bg-[var(--color-wardian-panel-header)]"
          >
            <span className="flex items-center gap-2">
              <span className="w-2 h-2 rounded-full bg-cyan-400 animate-pulse"></span>
              Active Runs ({activeRuns.length})
            </span>
            <span className={`transform transition-transform ${sectionOpen.activeRuns ? 'rotate-180' : ''}`}>▼</span>
          </button>
          
          {sectionOpen.activeRuns && (
            <div className="p-2 space-y-2">
              {activeRuns.length === 0 ? (
                <div className="text-xs text-muted text-center py-2 italic">No active runs</div>
              ) : (
                activeRuns.map(run => (
                  <div key={run.run_id} className="p-2 rounded bg-[var(--color-wardian-base)] border border-wardian-border text-sm flex flex-col gap-1.5">
                    <div className="flex justify-between items-center text-primary font-medium">
                      <span>{run.workflow_name}</span>
                      <span className="text-xs text-muted">Step {run.current_step} / {run.total_steps}</span>
                    </div>
                    <div className="w-full bg-[var(--color-wardian-surface-hover)] h-1.5 rounded-full overflow-hidden">
                      <div className="bg-cyan-500 h-full transition-all duration-300" style={{ width: `${(run.current_step / run.total_steps) * 100}%` }}></div>
                    </div>
                    <div className="flex justify-between items-center mt-1">
                      <span className="text-xs text-muted truncate max-w-[120px]" title={run.active_node_name}>
                        <span className="text-cyan-400 font-semibold mr-1">►</span> {run.active_node_name}
                      </span>
                      <div className="flex gap-1">
                        <button className="p-1 text-xs text-bright-neutral hover:text-cyan-400 transition-colors" title="View Trace"><EyeIcon /></button>
                        <button className="p-1 text-xs text-bright-neutral hover:text-red-400 transition-colors" title="Cancel Run"><StopIcon /></button>
                      </div>
                    </div>
                  </div>
                ))
              )}
            </div>
          )}
        </div>

        {/* Schedules Section */}
        <div className="border border-wardian-border rounded-lg bg-[var(--color-wardian-surface)] overflow-hidden">
          <button 
            onClick={() => toggleSection('schedules')}
            className="w-full flex items-center justify-between px-3 py-2 bg-[var(--color-wardian-surface-hover)] text-sm font-semibold text-primary transition-colors hover:bg-[var(--color-wardian-panel-header)]"
          >
            <span className="flex items-center gap-2">
              <ClockIcon />
              Schedules ({schedules.length})
            </span>
            <span className={`transform transition-transform ${sectionOpen.schedules ? 'rotate-180' : ''}`}>▼</span>
          </button>
          
          {sectionOpen.schedules && (
            <div className="p-2 space-y-2">
              {schedules.length === 0 ? (
                <div className="text-xs text-muted text-center py-2 italic">No active schedules</div>
              ) : (
                schedules.map(schedule => {
                   // Mock remaining time text
                   const diffMs = schedule.next_run_epoch_ms - Date.now();
                   const hrs = Math.floor(diffMs / 3600000);
                   const mins = Math.floor((diffMs % 3600000) / 60000);
                   const timeText = hrs > 0 ? `In ${hrs}h ${mins}m` : `In ${mins}m`;

                   return (
                    <div key={schedule.id} className="p-2 rounded bg-[var(--color-wardian-base)] border border-wardian-border text-sm flex justify-between items-center">
                      <div className="flex flex-col truncate max-w-[140px]">
                        <span className="text-primary font-medium truncate">{schedule.workflow_name}</span>
                        <span className="text-xs text-muted">{schedule.frequency}</span>
                      </div>
                      <div className="flex items-center gap-2">
                        <span className={`text-xs font-mono font-semibold ${schedule.is_paused ? 'text-amber-500' : 'text-emerald-400'}`}>
                          {schedule.is_paused ? 'PAUSED' : timeText}
                        </span>
                        <div className="flex gap-1">
                          <button className="p-1 text-xs text-bright-neutral hover:text-amber-400 transition-colors" title={schedule.is_paused ? "Resume" : "Pause"}>
                            {schedule.is_paused ? <PlayIcon /> : <PauseIcon />}
                          </button>
                          <button className="p-1 text-xs text-bright-neutral hover:text-red-400 transition-colors" title="Delete Schedule"><TrashIcon /></button>
                        </div>
                      </div>
                    </div>
                  );
                })
              )}
            </div>
          )}
        </div>

        {/* Workflows (Blueprints) Section */}
        <div className="border border-wardian-border rounded-lg bg-[var(--color-wardian-surface)] overflow-hidden">
          <button 
            onClick={() => toggleSection('workflows')}
            className="w-full flex items-center justify-between px-3 py-2 bg-[var(--color-wardian-surface-hover)] text-sm font-semibold text-primary transition-colors hover:bg-[var(--color-wardian-panel-header)]"
          >
            <span className="flex items-center gap-2">
              <span className="w-4 h-4 mt-0.5"><FolderIcon /></span>
              Blueprints ({workflows.length})
            </span>
            <span className={`transform transition-transform ${sectionOpen.workflows ? 'rotate-180' : ''}`}>▼</span>
          </button>
          
          {sectionOpen.workflows && (
            <div className="p-2 space-y-2">
              {workflows.length === 0 ? (
                <div className="text-xs text-muted text-center py-2 italic">No workflows found</div>
              ) : (
                workflows.map(wf => (
                  <div key={wf.id} className="p-2 rounded bg-[var(--color-wardian-base)] border border-wardian-border text-sm flex items-center justify-between group">
                    <div className="flex items-center gap-2 truncate flex-1 mr-2">
                      <button 
                        className="relative flex items-center justify-center" 
                        title={`Status: ${wf.trigger_status}`}
                      >
                        <span className={`w-3 h-3 rounded-full ${getStatusColor(wf.trigger_status)}`}></span>
                      </button>
                      <div className="text-bright-neutral" title={`Trigger: ${wf.trigger_type}`}>
                        {getTriggerIcon(wf.trigger_type)}
                      </div>
                      <span className="text-primary font-medium truncate">{wf.name}</span>
                    </div>
                    
                    {/* Action buttons appear on hover implicitly or are just faint without it. Using standard utility here. */}
                    <div className="flex gap-0.5 opacity-60 group-hover:opacity-100 transition-opacity">
                      <button className="p-1.5 rounded hover:bg-[var(--color-wardian-surface)] text-bright-neutral hover:text-cyan-400 transition-colors" title="Edit Workflow">
                        <EditIcon />
                      </button>
                      <button className="p-1.5 rounded hover:bg-[var(--color-wardian-surface)] text-bright-neutral hover:text-emerald-400 transition-colors" title="Run Now">
                        <PlayIcon />
                      </button>
                    </div>
                  </div>
                ))
              )}
            </div>
          )}
        </div>

      </div>
    </div>
  );
};
