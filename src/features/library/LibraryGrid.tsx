import React from 'react';
import { LibraryFolder, LibraryPrompt, LibrarySkill } from '../../types';
import { LibraryCard } from './LibraryCard';

interface LibraryGridProps {
    folder: LibraryFolder;
    hasSelectedAgents: boolean;
    onItemClick: (item: LibraryPrompt | LibrarySkill) => void;
    onToggleStar: (item: LibraryPrompt | LibrarySkill) => void;
    onFolderClick: (folder: LibraryFolder) => void;
    onItemAction: (item: LibraryPrompt | LibrarySkill) => Promise<void> | void;
}

export const LibraryGrid: React.FC<LibraryGridProps> = ({ folder, hasSelectedAgents, onItemClick, onToggleStar, onFolderClick, onItemAction }) => {
    return (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4 p-4">
            {folder.children.map((child, index) => {
                if ('metadata' in child) {
                    // It's a LibraryPrompt or LibrarySkill
                    const item = child as (LibraryPrompt | LibrarySkill);
                    return (
                        <LibraryCard 
                            key={`item-${item.path}-${index}`}
                            item={item}
                            hasSelectedAgents={hasSelectedAgents}
                            onClick={() => onItemClick(item)}
                            onToggleStar={(e) => {
                                e.stopPropagation();
                                onToggleStar(item);
                            }}
                            onAction={async (e) => {
                                e.stopPropagation();
                                await onItemAction(item);
                            }}
                        />
                    );
                } else {
                    // It's a LibraryFolder
                    return (
                        <div 
                            key={`folder-${child.path}-${index}`}
                            className="bg-wardian-card-bg border border-wardian-border rounded-xl p-4 flex items-center gap-3 cursor-pointer hover:border-[var(--color-wardian-accent)] transition-all group"
                            onClick={() => onFolderClick(child as LibraryFolder)}
                        >
                            <svg className="w-8 h-8 text-[var(--color-wardian-accent)] opacity-80 group-hover:opacity-100 transition-opacity" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.5" d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z"></path>
                            </svg>
                            <h3 className="text-primary font-bold text-lg truncate flex-1">
                                {(child as LibraryFolder).name}
                            </h3>
                        </div>
                    );
                }
            })}
            
            {folder.children.length === 0 && (
                <div className="col-span-full py-12 text-center text-muted-neutral">
                    <p>This folder is empty.</p>
                </div>
            )}
        </div>
    );
};
