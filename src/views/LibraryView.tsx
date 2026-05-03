import React, { useEffect, useState, useMemo } from 'react';
import { useLibraryStore } from '../store/useLibraryStore';
import { LibraryGrid } from '../features/library/LibraryGrid';
import { ItemEditorModal } from '../features/library/ItemEditorModal';
import { AssignSkillModal } from '../features/library/AssignSkillModal';
import { AssignPromptModal } from '../features/library/AssignPromptModal';
import { LibraryFolder, LibraryPrompt, LibrarySkill } from '../types';
import { flattenPromptForInjection, submitInputToAgents } from '../utils/terminalInput';

interface LibraryViewProps {
    selectedAgentIds: Set<string>;
}

export const LibraryView: React.FC<LibraryViewProps> = ({ selectedAgentIds }) => {
    const { promptTree, skillTree, isLoading, error, saveLibraryItem, updateLibraryMetadata, openLibraryFolder, activeTab, setActiveTab, subscribeToLibraryChanges } = useLibraryStore();

    // Navigation state
    const [currentPath, setCurrentPath] = useState<string[]>([]);

    // Editor state
    const [editingItem, setEditingItem] = useState<LibraryPrompt | LibrarySkill | null>(null);

    // Assign Skill Modal State
    const [assigningSkill, setAssigningSkill] = useState<LibrarySkill | null>(null);

    // Assign Prompt Modal State
    const [assigningPrompt, setAssigningPrompt] = useState<LibraryPrompt | null>(null);

    // Search & Filter
    const [searchQuery, setSearchQuery] = useState('');
    const [showQuickOnly, setShowQuickOnly] = useState(false);

    useEffect(() => {
        setCurrentPath([]);
        setSearchQuery('');
    }, [activeTab]);

    useEffect(() => {
        if (activeTab !== 'skills') return;
        return subscribeToLibraryChanges('skills');
    }, [activeTab, subscribeToLibraryChanges]);

    const currentFolder = useMemo(() => {
        const libraryTree = activeTab === 'prompts' ? promptTree : skillTree;
        if (!libraryTree) return null;
        let folder = libraryTree;
        for (const segment of currentPath) {
            const nextFolder = folder.children.find(c => !('content' in c) && !('description' in c) && c.name === segment) as LibraryFolder | undefined;
            if (nextFolder) {
                folder = nextFolder;
            } else {
                break;
            }
        }
        return folder;
    }, [promptTree, skillTree, activeTab, currentPath]);

    const filteredFolder = useMemo(() => {
        if (!currentFolder) return null;
        
        let children = currentFolder.children;
        
        if (searchQuery.trim() !== '') {
            const q = searchQuery.toLowerCase();
            children = children.filter(c => {
                if ('metadata' in c) {
                    const item = c as (LibraryPrompt | LibrarySkill);
                    const hasMatch = item.name.toLowerCase().includes(q) || 
                                     item.metadata.tags.some(t => t.toLowerCase().includes(q)) ||
                                     item.content.toLowerCase().includes(q);
                    if (hasMatch) return true;
                    if ('description' in item) {
                        return item.description.toLowerCase().includes(q);
                    }
                    return false;
                } else {
                    return c.name.toLowerCase().includes(q);
                }
            });
        }

        if (showQuickOnly) {
            children = children.filter(c => {
                if ('metadata' in c) {
                    return (c as (LibraryPrompt | LibrarySkill)).metadata.is_starred;
                }
                return true; // Keep folders visible so we can navigate to starred items inside
            });
        }

        return { ...currentFolder, children };
    }, [currentFolder, searchQuery, showQuickOnly]);

    if (isLoading && !(activeTab === 'prompts' ? promptTree : skillTree)) {
        return <div className="flex-1 flex items-center justify-center text-muted">Loading library...</div>;
    }

    if (error) {
        return <div className="flex-1 flex items-center justify-center text-red-500">Error: {error}</div>;
    }

    const handleItemAction = async (item: LibraryPrompt | LibrarySkill) => {
        if ('description' in item) {
            // Open Assign Skill Modal
            setAssigningSkill(item as LibrarySkill);
        } else {
            // Run Prompt
            if (selectedAgentIds.size === 0) {
                setAssigningPrompt(item as LibraryPrompt);
                return;
            }
            try {
                const flattenedPrompt = flattenPromptForInjection(item.content);
                await submitInputToAgents(selectedAgentIds, flattenedPrompt);
            } catch (e) {
                console.error('Failed to run prompt', e);
            }
        }
    };

    return (
        <div data-testid="library-view" className="flex-1 h-full flex flex-col bg-wardian-bg text-primary overflow-hidden">
            {/* Top Bar */}
            <div className="p-4 border-b border-wardian-border bg-wardian-sidebar-primary flex items-center gap-4">
                
                {/* Tabs */}
                <div className="flex gap-1 bg-wardian-bg p-1 rounded-lg border border-wardian-border">
                    <button 
                        onClick={() => setActiveTab('prompts')}
                        className={`px-4 py-1.5 rounded-md text-xs font-bold transition-all ${activeTab === 'prompts' ? 'bg-wardian-sidebar-primary shadow-sm text-primary' : 'text-muted hover:text-primary'}`}
                    >
                        Prompts
                    </button>
                    <button 
                        onClick={() => setActiveTab('skills')}
                        className={`px-4 py-1.5 rounded-md text-xs font-bold transition-all ${activeTab === 'skills' ? 'bg-wardian-sidebar-primary shadow-sm text-primary' : 'text-muted hover:text-primary'}`}
                    >
                        Skills
                    </button>
                </div>

                <div className="w-px h-6 bg-wardian-border mx-2"></div>

                {/* Breadcrumbs */}
                <div className="flex items-center gap-2 flex-1 label-small overflow-x-auto no-scrollbar">
                    <button 
                        className="text-muted hover:text-[var(--color-wardian-accent)] transition-colors whitespace-nowrap"
                        onClick={() => setCurrentPath([])}
                    >
                        library
                    </button>
                    <span className="text-muted-neutral">/</span>
                    <button 
                        className="text-muted hover:text-[var(--color-wardian-accent)] transition-colors whitespace-nowrap"
                        onClick={() => setCurrentPath([])}
                    >
                        {activeTab}
                    </button>
                    {currentPath.map((segment, idx) => (
                        <React.Fragment key={`crumb-${idx}`}>
                            <span className="text-muted-neutral">/</span>
                            <button 
                                className="text-muted hover:text-[var(--color-wardian-accent)] transition-colors whitespace-nowrap"
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
                        className="flex items-center gap-1.5 px-3 py-1.5 rounded-full border border-wardian-border label-small text-muted hover:text-primary hover:border-wardian-accent transition-all whitespace-nowrap"
                        title="Reveal Folder in File Explorer"
                    >
                        <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" />
                        </svg>
                        Reveal in Explorer
                    </button>
                    <input 
                        type="text" 
                        placeholder={`Search ${activeTab}...`}
                        value={searchQuery}
                        onChange={e => setSearchQuery(e.target.value)}
                        className="bg-[var(--color-wardian-input-bg)] border border-wardian-border rounded-full px-4 py-1.5 text-xs focus:outline-none focus:border-[var(--color-wardian-accent)] w-64 text-primary"
                    />
                    <button 
                        onClick={() => setShowQuickOnly(!showQuickOnly)}
                        className={`flex items-center gap-1.5 px-3 py-1.5 rounded-full border label-small transition-all whitespace-nowrap ${showQuickOnly ? 'bg-wardian-warning/10 border-wardian-warning/30 text-wardian-warning shadow-wardian-accent' : 'bg-transparent border-wardian-border text-muted hover:border-wardian-accent'}`}
                    >
                        <svg className="w-3.5 h-3.5" fill={showQuickOnly ? "currentColor" : "none"} stroke="currentColor" viewBox="0 0 24 24">
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M11.049 2.927c.3-.921 1.603-.921 1.902 0l1.519 4.674a1 1 0 00.95.69h4.915c.969 0 1.371 1.24.588 1.81l-3.976 2.888a1 1 0 00-.363 1.118l1.518 4.674c.3.922-.755 1.688-1.538 1.118l-3.976-2.888a1 1 0 00-1.176 0l-3.976 2.888c-.783.57-1.838-.197-1.538-1.118l1.518-4.674a1 1 0 00-.363-1.118l-3.976-2.888c-.784-.57-.38-1.81.588-1.81h4.914a1 1 0 00.951-.69l1.519-4.674z" />
                        </svg>
                        Starred
                    </button>
                </div>
            </div>

            {/* Grid Area */}
            <div className="flex-1 overflow-y-auto">
                {filteredFolder ? (
                    <LibraryGrid 
                        folder={filteredFolder}
                        onFolderClick={(f) => setCurrentPath([...currentPath, f.name])}
                        onItemClick={(p) => setEditingItem(p)}
                        onToggleStar={(p) => {
                            updateLibraryMetadata(p.path, {
                                ...p.metadata,
                                is_starred: !p.metadata.is_starred
                            });
                        }}
                        onItemAction={handleItemAction}
                    />
                ) : null}
            </div>

            {/* Modals */}
            {editingItem && (
                <ItemEditorModal 
                    item={editingItem}
                    isOpen={true}
                    onClose={() => setEditingItem(null)}
                    onSave={saveLibraryItem}
                />
            )}
            
            {assigningSkill && (
                <AssignSkillModal 
                    skill={assigningSkill}
                    isOpen={true}
                    onClose={() => setAssigningSkill(null)}
                />
            )}

            {assigningPrompt && (
                <AssignPromptModal
                    prompt={assigningPrompt}
                    isOpen={true}
                    onClose={() => setAssigningPrompt(null)}
                />
            )}
        </div>
    );
};
