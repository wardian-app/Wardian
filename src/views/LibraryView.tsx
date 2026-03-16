import React, { useEffect, useState, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useLibraryStore } from '../store/useLibraryStore';
import { LibraryGrid } from '../features/library/LibraryGrid';
import { ItemEditorModal } from '../features/library/ItemEditorModal';
import { LibraryFolder, LibraryPrompt } from '../types';

interface LibraryViewProps {
    selectedAgentIds: Set<string>;
}

export const LibraryView: React.FC<LibraryViewProps> = ({ selectedAgentIds }) => {
    const { libraryTree, isLoading, error, fetchLibraryTree, savePrompt, updatePromptMetadata, openLibraryFolder } = useLibraryStore();
    
    // Navigation state
    const [currentPath, setCurrentPath] = useState<string[]>([]);
    
    // Editor state
    const [editingPrompt, setEditingPrompt] = useState<LibraryPrompt | null>(null);

    // Search & Filter
    const [searchQuery, setSearchQuery] = useState('');
    const [showQuickOnly, setShowQuickOnly] = useState(false);

    useEffect(() => {
        fetchLibraryTree();
    }, [fetchLibraryTree]);

    const currentFolder = useMemo(() => {
        if (!libraryTree) return null;
        let folder = libraryTree;
        for (const segment of currentPath) {
            const nextFolder = folder.children.find(c => !('content' in c) && c.name === segment) as LibraryFolder | undefined;
            if (nextFolder) {
                folder = nextFolder;
            } else {
                break;
            }
        }
        return folder;
    }, [libraryTree, currentPath]);

    const filteredFolder = useMemo(() => {
        if (!currentFolder) return null;
        
        let children = currentFolder.children;
        
        if (searchQuery.trim() !== '') {
            const q = searchQuery.toLowerCase();
            children = children.filter(c => {
                if ('content' in c) {
                    return c.name.toLowerCase().includes(q) || 
                           c.metadata.tags.some(t => t.toLowerCase().includes(q)) ||
                           c.content.toLowerCase().includes(q);
                } else {
                    return c.name.toLowerCase().includes(q);
                }
            });
        }

        if (showQuickOnly) {
            children = children.filter(c => {
                if ('content' in c) {
                    return c.metadata.is_starred;
                }
                return true; // Keep folders visible so we can navigate to starred items inside
            });
        }

        return { ...currentFolder, children };
    }, [currentFolder, searchQuery, showQuickOnly]);

    if (isLoading && !libraryTree) {
        return <div className="flex-1 flex items-center justify-center text-muted">Loading library...</div>;
    }

    if (error) {
        return <div className="flex-1 flex items-center justify-center text-red-500">Error: {error}</div>;
    }

    return (
        <div className="flex-1 h-full flex flex-col bg-wardian-bg text-primary overflow-hidden">
            {/* Top Bar */}
            <div className="p-4 border-b border-wardian-border bg-wardian-sidebar-primary flex items-center gap-4">
                {/* Breadcrumbs */}
                <div className="flex items-center gap-2 flex-1 text-sm font-bold tracking-wide">
                    <button 
                        className="text-muted hover:text-[var(--color-wardian-accent)] transition-colors"
                        onClick={() => setCurrentPath([])}
                    >
                        Library
                    </button>
                    {currentPath.map((segment, idx) => (
                        <React.Fragment key={`crumb-${idx}`}>
                            <span className="text-muted-neutral">/</span>
                            <button 
                                className="text-muted hover:text-[var(--color-wardian-accent)] transition-colors"
                                onClick={() => setCurrentPath(currentPath.slice(0, idx + 1))}
                            >
                                {segment}
                            </button>
                        </React.Fragment>
                    ))}
                </div>

                {/* Filters & Actions */}
                <div className="flex items-center gap-3">
                    <button 
                        onClick={() => openLibraryFolder(currentPath.join('/'))}
                        className="flex items-center gap-1.5 px-3 py-1.5 rounded-full border border-wardian-light text-xs font-bold text-muted hover:text-primary hover:border-wardian-border transition-all"
                        title="Reveal Folder in File Explorer"
                    >
                        <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" />
                        </svg>
                        Reveal in Explorer
                    </button>
                    <input 
                        type="text" 
                        placeholder="Search prompts & tags..."
                        value={searchQuery}
                        onChange={e => setSearchQuery(e.target.value)}
                        className="bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded-full px-4 py-1.5 text-xs focus:outline-none focus:border-[var(--color-wardian-accent)] w-64"
                    />
                    <button 
                        onClick={() => setShowQuickOnly(!showQuickOnly)}
                        className={`flex items-center gap-1.5 px-3 py-1.5 rounded-full border text-xs font-bold transition-all ${showQuickOnly ? 'bg-yellow-400/10 border-yellow-400/30 text-yellow-400' : 'bg-transparent border-wardian-light text-muted hover:border-wardian-border'}`}
                    >
                        <svg className="w-3.5 h-3.5" fill={showQuickOnly ? "currentColor" : "none"} stroke="currentColor" viewBox="0 0 24 24">
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M11.049 2.927c.3-.921 1.603-.921 1.902 0l1.519 4.674a1 1 0 00.95.69h4.915c.969 0 1.371 1.24.588 1.81l-3.976 2.888a1 1 0 00-.363 1.118l1.518 4.674c.3.922-.755 1.688-1.538 1.118l-3.976-2.888a1 1 0 00-1.176 0l-3.976 2.888c-.783.57-1.838-.197-1.538-1.118l1.518-4.674a1 1 0 00-.363-1.118l-3.976-2.888c-.784-.57-.38-1.81.588-1.81h4.914a1 1 0 00.951-.69l1.519-4.674z" />
                        </svg>
                        Filter
                    </button>
                </div>
            </div>

            {/* Grid Area */}
            <div className="flex-1 overflow-y-auto">
                {filteredFolder ? (
                    <LibraryGrid 
                        folder={filteredFolder}
                        hasSelectedAgents={selectedAgentIds.size > 0}
                        onFolderClick={(f) => setCurrentPath([...currentPath, f.name])}
                        onPromptClick={(p) => setEditingPrompt(p)}
                        onToggleStar={(p) => {
                            updatePromptMetadata(p.path, {
                                ...p.metadata,
                                is_starred: !p.metadata.is_starred
                            });
                        }}
                        onRunPrompt={async (p) => {
                            if (selectedAgentIds.size === 0) {
                                console.warn("No agent selected to run prompt.");
                                return;
                            }
                            try {
                                const flattenedPrompt = p.content.replace(/\r?\n/g, ' ');
                                for (const id of selectedAgentIds) {
                                    await invoke('send_input_to_agent', { sessionId: id, input: flattenedPrompt });
                                }
                            } catch (e) {
                                console.error('Failed to run prompt', e);
                            }
                        }}
                    />
                ) : null}
            </div>

            {/* Modals */}
            {editingPrompt && (
                <ItemEditorModal 
                    prompt={editingPrompt}
                    isOpen={true}
                    onClose={() => setEditingPrompt(null)}
                    onSave={savePrompt}
                />
            )}
        </div>
    );
};
