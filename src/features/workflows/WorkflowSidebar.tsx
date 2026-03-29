import React, { useState, useMemo, useEffect } from 'react';
import { useWorkflowStore } from '../../store/useWorkflowStore';
import { WorkflowLibrary } from './WorkflowLibrary';
import { ActiveMonitoring } from './ActiveMonitoring';
import { RunPayloadModal, getManualTriggerSchema } from './RunPayloadModal';
import type { WorkflowDefinition } from '../../types/workflow';
import { useConfirm } from '../../components/ConfirmDialog';
import {
  buildScheduledRunFromWorkflow,
  getWorkflowLaunchKind,
  normalizeWorkflowForLaunch,
  setWorkflowTriggerStatus,
  workflowNeedsRunConfig,
  type WorkflowLaunchKind,
} from './workflowLaunch';

const StopAllIcon = () => <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 20 20"><path d="M5 5h10v10H5z" /></svg>;
const PauseAllIcon = () => <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 20 20"><path d="M5 4h3v12H5zm7 0h3v12h-3z" /></svg>;
const PlayAllIcon = () => <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 20 20"><path d="M4 4l12 6-12 6z" /></svg>;
const SearchIcon = () => <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth="2"><circle cx="11" cy="11" r="8"/><path d="M21 21l-4.35-4.35"/></svg>;

interface WorkflowSidebarProps {
  onOpenWorkflowBuilder?: () => void;
}

interface PendingWorkflowLaunch {
  workflow: WorkflowDefinition;
  kind: WorkflowLaunchKind;
}

export const WorkflowSidebar: React.FC<WorkflowSidebarProps> = ({ onOpenWorkflowBuilder }) => {
  const {
    availableWorkflows,
    fetchWorkflows,
    runWorkflowById,
    loadWorkflow,
    saveWorkflow,
    deleteWorkflow,
    stopAllTriggers,
    stopWorkflowTriggers,
    stopWorkflowRun,
    pauseAllTriggers,
    resumeAllTriggers,
    agents,
    activeRuns,
    scheduledRuns,
    loadScheduledRuns,
    createScheduledRun,
    toggleScheduledRun,
    deleteScheduledRun,
    runScheduledWorkflowNow,
  } = useWorkflowStore();

  const confirm = useConfirm();
  const [searchQuery, setSearchQuery] = useState('');
  const [pendingLaunch, setPendingLaunch] = useState<PendingWorkflowLaunch | null>(null);

  const executeWorkflowLaunch = async (
    workflow: WorkflowDefinition,
    kind: WorkflowLaunchKind,
    payload?: Record<string, any>,
  ) => {
    const mergedRoleMappings = payload?.role_mappings && typeof payload.role_mappings === 'object'
      ? { ...(workflow.role_mappings || {}), ...payload.role_mappings }
      : workflow.role_mappings || {};

    const configuredWorkflow = normalizeWorkflowForLaunch({
      ...workflow,
      role_mappings: mergedRoleMappings,
    });

    if (kind === 'scheduled') {
      const scheduledRun = buildScheduledRunFromWorkflow(configuredWorkflow);
      if (!scheduledRun) {
        return;
      }

      await createScheduledRun(scheduledRun);
      return;
    }

    if (kind === 'listener') {
      await saveWorkflow(setWorkflowTriggerStatus(configuredWorkflow, 'active'));
      await fetchWorkflows();
      return;
    }

    await runWorkflowById(configuredWorkflow.id, payload);
  };

  const handleRunFromLibrary = (id: string) => {
    const wf = availableWorkflows.find(w => w.id === id);
    if (!wf) return;

    const normalized = normalizeWorkflowForLaunch(wf);
    const kind = getWorkflowLaunchKind(normalized);
    const requiresConfig = workflowNeedsRunConfig(normalized, Boolean(getManualTriggerSchema(normalized)));

    if (requiresConfig) {
      setPendingLaunch({ workflow: normalized, kind });
      return;
    }

    void executeWorkflowLaunch(normalized, kind);
  };

  useEffect(() => { loadScheduledRuns(); }, [loadScheduledRuns]);

  const filteredWorkflows = useMemo(() => {
    if (!searchQuery.trim()) return availableWorkflows;
    return availableWorkflows.filter(wf =>
      wf.name.toLowerCase().includes(searchQuery.toLowerCase())
    );
  }, [availableWorkflows, searchQuery]);

  const activeWorkflows = useMemo(() =>
    availableWorkflows.filter(wf => {
      const triggerNode = wf.nodes.find(n => n.type === 'trigger');
      return triggerNode?.config?.status === 'active' && getWorkflowLaunchKind(wf) === 'listener';
    }),
  [availableWorkflows]);

  const handleStopAll = async () => {
    if (await confirm('STOP ALL: Immediately terminate all active runs and triggers?')) {
      await stopAllTriggers();
      fetchWorkflows();
    }
  };

  const handleDeleteSchedule = async (id: string) => {
    if (await confirm('Delete this scheduled task? This removes only this scheduled instance.')) {
      await deleteScheduledRun(id);
    }
  };

  return (
    <div className="flex flex-col h-full select-none">
      <div className="flex flex-col gap-2 mb-1">
        <div className="flex justify-between items-center">
          <h2 className="text-xl font-bold text-primary tracking-tight truncate">
            Workflows
          </h2>
        </div>

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

        <div className="flex bg-[var(--color-wardian-input-bg)] border border-wardian-border rounded-lg p-0.5 overflow-hidden shadow-sm">
          <button
            onClick={() => resumeAllTriggers()}
            className="flex-1 flex justify-center items-center py-1 text-muted-neutral hover:text-wardian-success hover:bg-wardian-success/10 rounded-md transition-all group"
            title="Resume All"
          >
            <PlayAllIcon />
          </button>
          <div className="w-px bg-wardian-border my-1 mx-0.5" />
          <button
            onClick={() => pauseAllTriggers()}
            className="flex-1 flex justify-center items-center py-1 text-muted-neutral hover:text-wardian-warning hover:bg-wardian-warning/10 rounded-md transition-all group"
            title="Pause All"
          >
            <PauseAllIcon />
          </button>
          <div className="w-px bg-wardian-border my-1 mx-0.5" />
          <button
            onClick={handleStopAll}
            className="flex-1 flex justify-center items-center py-1 text-muted-neutral hover:text-wardian-error hover:bg-wardian-error/10 rounded-md transition-all group"
            title="Stop All (Panic)"
          >
            <StopAllIcon />
          </button>
        </div>
      </div>

      <div className="flex-1 flex flex-col min-h-0 overflow-y-auto no-scrollbar pr-1 -mr-1">
        <ActiveMonitoring
          activeRuns={activeRuns}
          schedules={scheduledRuns}
          activeWorkflows={activeWorkflows}
          availableWorkflows={availableWorkflows}
          agents={agents}
          onStopRun={(id) => stopWorkflowRun(id)}
          onStopTrigger={(id) => { stopWorkflowTriggers(id); fetchWorkflows(); }}
          onToggleSchedule={(id) => toggleScheduledRun(id)}
          onDeleteSchedule={handleDeleteSchedule}
          onRunNow={(scheduleId) => { runScheduledWorkflowNow(scheduleId); }}
          onOpenWorkflow={(workflowId) => {
            const wf = availableWorkflows.find(w => w.id === workflowId);
            if (wf) loadWorkflow(wf);
            onOpenWorkflowBuilder?.();
          }}
        />

        <div className="my-2 border-t border-wardian-border/20" />

        <div className="flex-1 flex flex-col min-h-0">
          <div className="mb-1 label-small">
            Library
          </div>
          <WorkflowLibrary
            workflows={filteredWorkflows.map(wf => {
              const triggerNode = wf.nodes.find(n => n.type === 'trigger');
              let trigger_type = 'manual';
              if (triggerNode?.name === 'Scheduled Trigger') trigger_type = 'scheduled';
              else if (triggerNode?.name === 'File Watcher') trigger_type = 'watcher';
              else if (triggerNode?.config?.type === 'Webhook') trigger_type = 'webhook';
              return {
                ...wf,
                trigger_type,
                trigger_status: triggerNode?.config?.status || 'off'
              };
            })}
            onRun={handleRunFromLibrary}
            onEdit={(id) => {
              const wf = availableWorkflows.find(w => w.id === id);
              if (wf) loadWorkflow(wf);
              onOpenWorkflowBuilder?.();
            }}
            onDelete={deleteWorkflow}
          />
        </div>
      </div>

      {pendingLaunch && (
        <RunPayloadModal
          workflow={pendingLaunch.workflow}
          isOpen={true}
          agents={agents.map(a => ({ session_id: a.session_id, session_name: a.session_name }))}
          onRun={async (payload) => {
            await executeWorkflowLaunch(pendingLaunch.workflow, pendingLaunch.kind, payload);
            setPendingLaunch(null);
          }}
          onCancel={() => setPendingLaunch(null)}
        />
      )}
    </div>
  );
};
