import React, { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { FileTree, FileNode } from './FileTree';

interface ExplorerPanelProps {
  selectedAgentIds: Set<string>;
  agents: any[];
}

export const ExplorerPanel: React.FC<ExplorerPanelProps> = ({ selectedAgentIds, agents }) => {
  const [rootPath, setRootPath] = useState<string | null>(null);
  
  // Context Menu State
  const [menuPos, setMenuPos] = useState<{x: number, y: number} | null>(null);
  const [activeNode, setActiveNode] = useState<FileNode | null>(null);
  
  // Preview Modal State
  const [previewContent, setPreviewContent] = useState<string | null>(null);
  const [previewTitle, setPreviewTitle] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);

  const selectedAgentId = selectedAgentIds.size === 1 ? Array.from(selectedAgentIds)[0] : null;
  const selectedAgentName = selectedAgentId ? agents.find(a => a.session_id === selectedAgentId)?.session_name : null;

  useEffect(() => {
    const fetchPath = async () => {
      try {
        const path = await invoke<string>('get_explorer_root', { sessionId: selectedAgentId });
        setRootPath(path);
      } catch (err) {
        console.error("Failed to fetch root path", err);
      }
    };
    fetchPath();
  }, [selectedAgentId]);

  useEffect(() => {
    const handleClick = () => setMenuPos(null);
    window.addEventListener('click', handleClick);
    return () => window.removeEventListener('click', handleClick);
  }, []);

  const handleContextMenu = (e: React.MouseEvent, node: FileNode) => {
    e.preventDefault();
    setMenuPos({ x: e.clientX, y: e.clientY });
    setActiveNode(node);
  };

  const handleCopyPath = async () => {
    if (activeNode) {
      await writeText(activeNode.path);
    }
    setMenuPos(null);
  };

  const handleReveal = async () => {
    if (activeNode) {
      try {
        await invoke('reveal_in_explorer', { path: activeNode.path });
      } catch (err) {
        console.error("Reveal failed:", err);
      }
    }
    setMenuPos(null);
  };

  const handlePreview = async () => {
    if (activeNode && !activeNode.is_dir) {
      try {
        const content = await invoke<string>('read_file_preview', { path: activeNode.path });
        setPreviewTitle(activeNode.name);
        setPreviewContent(content);
      } catch (err) {
        console.error("Preview failed:", err);
        setPreviewTitle("Error reading " + activeNode.name);
        setPreviewContent(String(err));
      }
    }
    setMenuPos(null);
  };

  const handleDelete = async () => {
    if (activeNode) {
      if (window.confirm(`Are you sure you want to delete ${activeNode.name}?`)) {
        try {
          await invoke('delete_file', { path: activeNode.path });
          setRefreshKey(prev => prev + 1); // trigger remount of root FileTree
        } catch (err) {
          console.error("Delete failed:", err);
          alert(`Failed to delete: ${err}`);
        }
      }
    }
    setMenuPos(null);
  };

  return (
    <div className="flex flex-col h-full w-full relative">
      <div className="flex flex-col mb-4 shrink-0 px-2 mt-4">
        <h2 className="text-xl font-bold text-primary tracking-tight">File Explorer</h2>
        <p className="text-xs text-muted mt-1 select-none">
          {selectedAgentName ? `Agent: ${selectedAgentName}` : 'Global Workspace'}
        </p>
      </div>
      
      <div className="flex-1 overflow-y-auto w-full no-scrollbar relative min-h-0 bg-wardian-darker rounded-xl border border-wardian-border p-2 mb-4">
        {rootPath ? (
          <FileTree 
            key={refreshKey}
            path={rootPath} 
            onContextMenu={handleContextMenu} 
          />
        ) : (
          <div className="text-sm text-wardian-text-muted p-2 animate-pulse">Mapping directory...</div>
        )}
      </div>

      {menuPos && activeNode && (
        <div 
          className="fixed bg-wardian-card border border-wardian-border shadow-xl rounded-md py-1 z-50 min-w-40 text-sm font-medium animate-in fade-in zoom-in-95 duration-100"
          style={{ 
            top: Math.min(menuPos.y, window.innerHeight - 200),
            left: Math.min(menuPos.x, window.innerWidth - 200) 
          }}
          onClick={(e) => e.stopPropagation()}
        >
          <div className="px-4 py-2 text-xs text-wardian-text-muted truncate border-b border-wardian-border tracking-wider font-semibold mb-1 max-w-64">
            {activeNode.name}
          </div>
          {!activeNode.is_dir && (
            <button className="w-full text-left px-4 py-2 hover:bg-wardian-card-bg-muted transition-colors text-wardian-text" onClick={handlePreview}>
              Open Preview
            </button>
          )}
          <button className="w-full text-left px-4 py-2 hover:bg-wardian-card-bg-muted transition-colors text-wardian-text" onClick={handleReveal}>
            Reveal in OS
          </button>
          <button className="w-full text-left px-4 py-2 hover:bg-wardian-card-bg-muted transition-colors text-wardian-text" onClick={handleCopyPath}>
            Copy Absolute Path
          </button>
          <div className="h-px bg-wardian-border my-1 w-full" />
          <button className="w-full text-left px-4 py-2 hover:bg-wardian-card-bg-muted transition-colors text-red-500 group flex items-center justify-between" onClick={handleDelete}>
            Delete
          </button>
        </div>
      )}

      {/* Preview Modal */}
      {previewContent !== null && (
        <div className="fixed inset-0 z-[100] bg-black/60 flex items-center justify-center p-8 backdrop-blur-sm animate-in fade-in" onClick={() => setPreviewContent(null)}>
          <div 
            className="bg-wardian-card-bg border border-wardian-border shadow-2xl rounded-xl w-full max-w-4xl max-h-[90vh] flex flex-col font-mono text-sm overflow-hidden animate-in zoom-in-95 duration-200"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex justify-between items-center p-4 border-b border-wardian-border shrink-0 bg-wardian-bg">
              <h3 className="font-bold text-lg text-wardian-accent truncate flex-1 mr-4">{previewTitle}</h3>
              <button onClick={() => setPreviewContent(null)} className="text-wardian-text-muted hover:text-red-400 font-bold transition-colors w-8 h-8 flex items-center justify-center rounded-md hover:bg-red-400/10">✕</button>
            </div>
            <div className="p-0 overflow-y-auto flex-1 bg-[#1e1e1e] cursor-text">
              <pre className="p-6 text-[#d4d4d4] whitespace-pre-wrap break-words">{previewContent}</pre>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
