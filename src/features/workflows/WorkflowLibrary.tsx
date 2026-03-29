import React, { useState } from 'react';
import { useWorkflowLibrary } from './useWorkflowLibrary';
import { ContextMenu, ContextMenuItem } from '../../components/ContextMenu';
import { useConfirm } from '../../components/ConfirmDialog';

// Icons

const ScheduleIcon = () => (
  <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
    <circle cx="12" cy="12" r="9" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" />
    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 7v5l3 2" />
  </svg>
);

const WatcherIcon = () => (
  <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M13.828 10.172a4 4 0 00-5.656 0l-4 4a4 4 0 105.656 5.656l1.102-1.101m-.758-4.899a4 4 0 005.656 0l4-4a4 4 0 00-5.656-5.656l-1.1 1.1" />
  </svg>
);

const ManualIcon = () => (
  <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M13 10V3L4 14h7v7l9-11h-7z" />
  </svg>
);

const WebhookIcon = () => (
  <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M21 12a9 9 0 01-9 9m9-9a9 9 0 00-9-9m9 9H3m9 9a9 9 0 01-9-9m9 9c1.657 0 3-4.03 3-9s-1.343-9-3-9m0 18c-1.657 0-3-4.03-3-9s1.343-9 3-9m-9 9a9 9 0 019-9" />
  </svg>
);

const PlayIcon = () => <svg className="w-3.5 h-3.5" fill="currentColor" viewBox="0 0 20 20"><path d="M4 4l12 6-12 6z" /></svg>;

interface WorkflowLibraryProps {
  workflows: any[];
  onRun: (id: string) => void;
  onEdit: (id: string) => void;
  onDelete: (id: string) => void;
}

export const WorkflowLibrary: React.FC<WorkflowLibraryProps> = ({ workflows, onRun, onEdit, onDelete }) => {
  const confirm = useConfirm();
  const { folders, rootWorkflowIds, toggleFolderCollapse, moveWorkflowToFolder, addFolder, renameFolder, deleteFolder } = useWorkflowLibrary();
  const [contextMenu, setContextMenu] = useState<{ x: number, y: number, items: ContextMenuItem[] } | null>(null);
  const [draggedWorkflowId, setDraggedWorkflowId] = useState<string | null>(null);
  const [dragOverWorkflowId, setDragOverWorkflowId] = useState<string | null>(null);
  const wasDragging = React.useRef(false);

  // Cancel drag if mouse leaves the list area
  React.useEffect(() => {
    const handleGlobalMouseUp = () => {
      if (draggedWorkflowId) {
        setDraggedWorkflowId(null);
        setDragOverWorkflowId(null);
      }
    };
    window.addEventListener("mouseup", handleGlobalMouseUp);
    return () => window.removeEventListener("mouseup", handleGlobalMouseUp);
  }, [draggedWorkflowId]);

  const handleDragMouseUp = async () => {
    if (draggedWorkflowId && dragOverWorkflowId && draggedWorkflowId !== dragOverWorkflowId) {
      // Find the target folder and index from the dragOverWorkflowId
      let targetFolderId: string | null = null;
      let targetIndex = -1;

      for (const folder of folders) {
        const idx = folder.workflowIds.indexOf(dragOverWorkflowId);
        if (idx !== -1) {
          targetFolderId = folder.id;
          targetIndex = idx;
          break;
        }
      }

      if (targetIndex === -1) {
        targetIndex = rootWorkflowIds.indexOf(dragOverWorkflowId);
      }

      if (targetIndex !== -1) {
        await moveWorkflowToFolder(draggedWorkflowId, targetFolderId, targetIndex);
        wasDragging.current = true;
      }
    }
    setDraggedWorkflowId(null);
    setDragOverWorkflowId(null);
  };

  const getWorkflowById = (id: string) => workflows.find(w => w.id === id);

  const handleContextMenu = (e: React.MouseEvent, type: 'workflow' | 'folder' | 'empty', targetId?: string) => {
    e.preventDefault();
    e.stopPropagation();

    let items: ContextMenuItem[] = [];

    if (type === 'workflow') {
      items = [
        { label: 'Run Now', icon: <PlayIcon />, onClick: () => onRun(targetId!) },
        { label: 'Edit on Canvas', onClick: () => onEdit(targetId!) },
        { divider: true },
        { label: 'Delete', danger: true, onClick: () => onDelete(targetId!) },
      ];
    } else if (type === 'folder') {
      items = [
        { label: 'New Workflow in Folder', onClick: () => console.log('New WF in Folder', targetId) },
        { label: 'Rename Folder', onClick: () => {
          const name = prompt('New Folder Name:');
          if (name) renameFolder(targetId!, name);
        }},
        { label: 'Pause All Triggers in Folder', onClick: () => console.log('Pause All in Folder', targetId) },
        { label: 'Delete Folder', danger: true, onClick: async () => {
          if (await confirm('Delete folder and all workflows within?')) deleteFolder(targetId!);
        }},
      ];
    } else {
      items = [
        { label: 'New Workflow', onClick: () => console.log('New WF') },
        { label: 'New Folder', onClick: () => {
          const name = prompt('Folder Name:');
          if (name) addFolder(name);
        }},
        { divider: true },
        { label: 'Expand All', onClick: () => console.log('Expand All') },
        { label: 'Collapse All', onClick: () => console.log('Collapse All') },
      ];
    }

    setContextMenu({ x: e.clientX, y: e.clientY, items });
  };

  const renderWorkflowItem = (id: string) => {
    const wf = getWorkflowById(id);
    if (!wf) return null;

    const triggerType = wf.trigger_type || 'manual';
    const status = wf.trigger_status || 'off'; // active, muted, off

    const getGlyph = () => {
      switch (triggerType) {
        case 'scheduled': return <ScheduleIcon />;
        case 'watcher': return <WatcherIcon />;
        case 'webhook': return <WebhookIcon />;
        default: return <ManualIcon />;
      }
    };

    const statusColorClass = triggerType === 'scheduled' ? 'text-cyan-400' : status === 'active' ? 'text-emerald-500' : status === 'muted' ? 'text-amber-500' : 'text-gray-500';
    const isDragTarget = dragOverWorkflowId === wf.id && draggedWorkflowId !== null && draggedWorkflowId !== wf.id;
    const isBeingDragged = draggedWorkflowId === wf.id;

    return (
      <div 
        key={wf.id}
        onMouseDown={() => setDraggedWorkflowId(wf.id)}
        onMouseEnter={() => {
          if (draggedWorkflowId && draggedWorkflowId !== wf.id) {
            setDragOverWorkflowId(wf.id);
          }
        }}
        onMouseUp={(e) => {
          e.stopPropagation();
          handleDragMouseUp();
        }}
        onDoubleClick={() => onRun(wf.id)}
        onContextMenu={(e) => handleContextMenu(e, 'workflow', wf.id)}
        onClick={(e) => {
          if (wasDragging.current) {
            wasDragging.current = false;
            e.stopPropagation();
          }
        }}
        className={`flex items-center justify-between px-2 py-1.5 rounded-md hover:bg-white/5 group cursor-grab active:cursor-grabbing transition-all ${isBeingDragged ? 'opacity-40 grayscale' : ''} ${isDragTarget ? 'bg-white/10' : ''}`}
      >
        <div className="flex items-center gap-2 truncate pointer-events-none">
          <span className={statusColorClass}>
            {getGlyph()}
          </span>
          <span className="text-[11px] font-medium text-primary truncate tracking-tight">{wf.name}</span>
        </div>
        <button 
          onClick={(e) => { e.stopPropagation(); onRun(wf.id); }}
          className="opacity-0 group-hover:opacity-100 p-1 text-muted-neutral hover:text-[var(--color-wardian-accent)] transition-all hover:scale-110 pointer-events-auto"
        >
          <PlayIcon />
        </button>
      </div>
    );
  };

  return (
    <div 
      className="flex-1 overflow-y-auto no-scrollbar pb-10"
      onContextMenu={(e) => handleContextMenu(e, 'empty')}
      onMouseUp={() => {
        if (draggedWorkflowId) {
          moveWorkflowToFolder(draggedWorkflowId, null);
          setDraggedWorkflowId(null);
          setDragOverWorkflowId(null);
        }
      }}
    >
      {/* Folders */}
      {folders.map(folder => (
        <div 
          key={folder.id} 
          className="mb-1"
          onMouseEnter={() => {
            if (draggedWorkflowId) setDragOverWorkflowId(null); // Highlighting folder as target if needed
          }}
          onMouseUp={(e) => {
            if (draggedWorkflowId) {
              e.stopPropagation();
              moveWorkflowToFolder(draggedWorkflowId, folder.id);
              setDraggedWorkflowId(null);
              setDragOverWorkflowId(null);
            }
          }}
        >
          <div 
            onClick={() => toggleFolderCollapse(folder.id)}
            onContextMenu={(e) => handleContextMenu(e, 'folder', folder.id)}
            className="flex items-center gap-2 px-2 py-1.5 mb-0.5 text-[10px] font-bold text-muted hover:text-primary cursor-pointer transition-colors group"
          >
            <span className={`text-[8px] transition-transform duration-200 opacity-30 ${folder.isCollapsed ? '-rotate-90' : 'rotate-0'}`}>▼</span>
            <span className="truncate">{folder.name}</span>
            <span className="text-[9px] opacity-40 ml-auto font-mono group-hover:opacity-100">{folder.workflowIds.length}</span>
          </div>
          {!folder.isCollapsed && (
            <div className="ml-3 border-l border-wardian-border/20 pl-1 space-y-0.5 animate-in slide-in-from-left-1 duration-200">
              {folder.workflowIds.length === 0 ? (
                <div className="text-[9px] text-muted-neutral italic py-2 pl-4 opacity-50">Empty folder</div>
              ) : (
                folder.workflowIds.map((id) => renderWorkflowItem(id))
              )}
            </div>
          )}
        </div>
      ))}

      {/* Root Workflows */}
      {rootWorkflowIds.length > 0 && (
        <div className="space-y-0.5 mt-2">
          {rootWorkflowIds.map((id) => renderWorkflowItem(id))}
        </div>
      )}

      {/* Fallback for unorganized workflows (if any are missing from persistence) */}
      {workflows.filter(wf => wf && !rootWorkflowIds.includes(wf.id) && !folders.some(f => f.workflowIds.includes(wf.id))).map((wf) => (
        renderWorkflowItem(wf.id)
      ))}

      {contextMenu && (
        <ContextMenu 
          x={contextMenu.x}
          y={contextMenu.y}
          items={contextMenu.items}
          onClose={() => setContextMenu(null)}
        />
      )}
    </div>
  );
};

