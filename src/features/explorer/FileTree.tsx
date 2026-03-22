import React, { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ChevronRight, ChevronDown, Folder, File, FileText, Image, Code } from 'lucide-react';

export interface FileNode {
  name: string;
  path: string;
  is_dir: boolean;
  extension: string | null;
}

export interface FileTreeProps {
  path: string;
  onSelect?: (path: string, is_dir: boolean) => void;
  onContextMenu?: (e: React.MouseEvent, node: FileNode) => void;
  depth?: number;
}

const getFileIcon = (extension: string | null) => {
  if (!extension) return <File className="w-4 h-4 text-wardian-text-muted shrink-0" />;
  const ext = extension.toLowerCase();
  if (['png', 'jpg', 'jpeg', 'gif', 'svg', 'webp'].includes(ext)) {
    return <Image className="w-4 h-4 text-blue-400 shrink-0" />;
  }
  if (['ts', 'tsx', 'js', 'jsx', 'json', 'rs', 'py', 'html', 'css', 'md'].includes(ext)) {
    return <Code className="w-4 h-4 text-yellow-500 shrink-0" />;
  }
  return <FileText className="w-4 h-4 text-wardian-text-muted shrink-0" />;
}

export const FileTree: React.FC<FileTreeProps> = ({ path, onSelect, onContextMenu, depth = 0 }) => {
  const [nodes, setNodes] = useState<FileNode[]>([]);
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let isMounted = true;
    const fetchTree = async () => {
      setLoading(true);
      try {
        const result = await invoke<FileNode[]>('get_directory_tree', { path });
        if (isMounted) {
          setNodes(result);
          setError(null);
        }
      } catch (err) {
        if (isMounted) {
          setError(String(err));
          console.error("Failed to load directory tree for", path, err);
        }
      } finally {
        if (isMounted) setLoading(false);
      }
    };
    fetchTree();
    return () => { isMounted = false; };
  }, [path]);

  const toggleFolder = (nodePath: string) => {
    setExpanded(prev => ({ ...prev, [nodePath]: !prev[nodePath] }));
  };

  const handleClick = (e: React.MouseEvent, node: FileNode) => {
    e.stopPropagation();
    if (node.is_dir) {
      toggleFolder(node.path);
    }
    if (onSelect) {
      onSelect(node.path, node.is_dir);
    }
  };

  if (loading && depth === 0) {
    return <div className="text-sm text-wardian-text-muted p-2 animate-pulse">Loading workspace...</div>;
  }

  if (error && depth === 0) {
    return <div className="text-sm text-red-400 p-2 break-words">Error: {error}</div>;
  }

  return (
    <div className={`flex flex-col ${depth === 0 ? 'w-full h-full' : ''}`}>
      {nodes.map(node => (
        <React.Fragment key={node.path}>
          <div 
            className="flex items-center gap-1.5 py-1 px-2 hover:bg-wardian-card-bg-muted cursor-pointer rounded-md text-sm whitespace-nowrap overflow-hidden select-none group w-full"
            style={{ paddingLeft: `${(depth * 12) + 8}px` }}
            onClick={(e) => handleClick(e, node)}
            onContextMenu={(e) => onContextMenu && onContextMenu(e, node)}
          >
            {node.is_dir ? (
              <span className="text-wardian-text-muted cursor-pointer hover:text-wardian-text shrink-0 flex items-center justify-center w-4 h-4" onClick={(e) => { e.stopPropagation(); toggleFolder(node.path); }}>
                {expanded[node.path] ? <ChevronDown className="w-3 h-3" /> : <ChevronRight className="w-3 h-3" />}
              </span>
            ) : (
              <span className="w-4 h-4 shrink-0 inline-block" />
            )}
            
            <span className="text-wardian-text-muted flex items-center shrink-0">
              {node.is_dir ? (
                <Folder className={`w-4 h-4 ${expanded[node.path] ? 'fill-blue-500 text-blue-500' : 'text-blue-400'}`} />
              ) : (
                getFileIcon(node.extension)
              )}
            </span>
            
            <span className={`truncate flex-1 ${node.is_dir ? 'text-wardian-text font-medium' : 'text-wardian-text-muted group-hover:text-wardian-text'} transition-colors`}>
              {node.name}
            </span>
          </div>

          {node.is_dir && expanded[node.path] && (
            <FileTree 
              path={node.path} 
              depth={depth + 1} 
              onSelect={onSelect} 
              onContextMenu={onContextMenu} 
            />
          )}
        </React.Fragment>
      ))}
    </div>
  );
};
