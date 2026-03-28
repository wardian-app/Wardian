import React, { useState, useMemo } from 'react';
import { useWorkflowStore } from '../../store/useWorkflowStore';
import { WorkflowLibrary } from './WorkflowLibrary';
import { ActiveMonitoring } from './ActiveMonitoring';
import { useConfirm } from '../../components/ConfirmDialog';

// Icons
const StopAllIcon = () => <svg className="w-5 h-5" fill="currentColor" viewBox="0 0 20 20"><path d="M5 5h10v10H5z" /></svg>;
const PauseAllIcon = () => <svg className="w-5 h-5" fill="currentColor" viewBox="0 0 20 20"><path d="M5 4h3v12H5zm7 0h3v12h-3z" /></svg>;
const PlayAllIcon = () => <svg className="w-5 h-5" fill="currentColor" viewBox="0 0 20 20"><path d="M4 4l12 6-12 6z" /></svg>;
const SearchIcon = () => <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth="2"><circle cx="11" cy="11" r="8"/><path d="M21 21l-4.35-4.35"/></svg>;

interface WorkflowSidebarProps {}

export const WorkflowSidebar: React.FC<WorkflowSidebarProps> = () => {
  const { 
    availableWorkflows, 
    fetchWorkflows, 
    runWorkflowById, 
    loadWorkflow,
    deleteWorkflow,
    stopAllTriggers,
    pauseAllTriggers,
    resumeAllTriggers,
    activeRuns,
    schedules
  } = useWorkflowStore();
  
  const confirm = useConfirm();
  const [searchQuery, setSearchQuery] = useState('');

  const filteredWorkflows = useMemo(() => {
    if (!searchQuery.trim()) return availableWorkflows;
    return availableWorkflows.filter(wf => 
      wf.name.toLowerCase().includes(searchQuery.toLowerCase())
    );
  }, [availableWorkflows, searchQuery]);

  const activeWorkflows = useMemo(() => 
    availableWorkflows.filter(wf => {
      const triggerNode = wf.nodes.find(n => n.type === 'trigger');
      return triggerNode?.config?.status === 'active';
    }),
  [availableWorkflows]);

  const handleStopAll = async () => {
    if (await confirm('STOP ALL: Immediately terminate all active runs and triggers?')) {
      await stopAllTriggers();
      fetchWorkflows();
    }
  };

  return (
    <div className="flex flex-col h-full select-none">
      {/* Refined Workflow Sidebar Header */}
      <div className="flex flex-col gap-3 mb-6">
        {/* Title Row */}
        <div className="flex justify-between items-center">
          <h2 className="text-xl font-bold text-primary tracking-tight truncate">
            Workflows
          </h2>
        </div>

        {/* Search Bar */}
        <div className="relative">
          <div className="absolute left-3 top-1/2 -translate-y-1/2 text-muted-neutral pointer-events-none">
            <SearchIcon />
          </div>
          <input 
            type="text" 
            placeholder="Search workflows..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded-lg pl-9 pr-3 py-1.5 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)] transition-all placeholder:text-muted-neutral"
          />
        </div>

        {/* Governance Controls (Centered) */}
        <div className="flex justify-center items-center gap-4 py-1 border-b border-wardian-border/20 pb-4">
          <button 
            onClick={handleStopAll}
            className="p-2 rounded-lg bg-wardian-error/10 hover:bg-wardian-error/20 text-wardian-error border border-wardian-error/20 transition-all hover:scale-110 active:scale-95"
            title="Stop All (Panic)"
          >
            <StopAllIcon />
          </button>
          <button 
            onClick={() => pauseAllTriggers()}
            className="p-2 rounded-lg bg-wardian-warning/10 hover:bg-wardian-warning/20 text-wardian-warning border border-wardian-warning/20 transition-all hover:scale-110 active:scale-95"
            title="Pause All"
          >
            <PauseAllIcon />
          </button>
          <button 
            onClick={() => resumeAllTriggers()}
            className="p-2 rounded-lg bg-wardian-success/10 hover:bg-wardian-success/20 text-wardian-success border border-wardian-success/20 transition-all hover:scale-110 active:scale-95"
            title="Resume All"
          >
            <PlayAllIcon />
          </button>
        </div>
      </div>

      <div className="flex-1 flex flex-col min-h-0 overflow-y-auto no-scrollbar pr-1 -mr-1">
        {/* Monitoring Area */}
        <ActiveMonitoring 
          activeRuns={activeRuns}
          schedules={schedules}
          activeWorkflows={activeWorkflows}
          onStopRun={(id) => console.log('Stop Run', id)}
          onStopTrigger={(id) => console.log('Stop Trigger', id)}
          onToggleSchedule={(id) => console.log('Toggle Schedule', id)}
        />

        <div className="my-4 border-t border-wardian-border/20" />

        {/* 1. Workflows (The Library) */}
        <div className="flex-1 flex flex-col min-h-0">
          <div className="mb-4 label-small">
            Library
          </div>
          <WorkflowLibrary 
            workflows={filteredWorkflows.map(wf => {
              const triggerNode = wf.nodes.find(n => n.type === 'trigger');
              return {
                ...wf,
                trigger_type: triggerNode?.config?.type || 'manual',
                trigger_status: triggerNode?.config?.status || 'off'
              };
            })}
            onRun={runWorkflowById}
            onEdit={(id) => {
              const wf = availableWorkflows.find(w => w.id === id);
              if (wf) loadWorkflow(wf);
            }}
            onDelete={deleteWorkflow}
          />
        </div>
      </div>
    </div>
  );
};
