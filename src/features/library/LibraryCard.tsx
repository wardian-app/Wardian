import React, { useState } from 'react';
import { LibraryPrompt, LibrarySkill } from '../../types';

interface LibraryCardProps {
    item: LibraryPrompt | LibrarySkill;
    onClick: () => void; // This is now used for editing
    onToggleStar: (e: React.MouseEvent) => void;
    onAction: (e: React.MouseEvent) => Promise<void> | void; // This is now the main card click
}

export const LibraryCard: React.FC<LibraryCardProps> = ({ item, onClick, onToggleStar, onAction }) => {
    const [isActioning, setIsActioning] = useState(false);

    const handleMainClick = async (e: React.MouseEvent) => {
        setIsActioning(true);
        try {
            await onAction(e);
        } finally {
            setIsActioning(false);
        }
    };

    const handleEditClick = (e: React.MouseEvent) => {
        e.stopPropagation();
        onClick();
    };

    return (
        <div 
            className="bg-wardian-card-bg border border-wardian-border rounded-xl p-4 flex flex-col gap-3 transition-all group relative cursor-pointer hover:border-[var(--color-wardian-accent)] hover:shadow-sm"
            onClick={handleMainClick}
        >
            <div className="flex justify-between items-start">
                <h3 className="text-primary font-bold text-lg truncate flex items-center gap-2" title={item.name}>
                    {item.name}
                    {isActioning && (
                        <svg className="w-4 h-4 text-wardian-success animate-spin flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor">
                            <circle cx="12" cy="12" r="10" strokeWidth="4" strokeOpacity="0.25"></circle>
                            <path d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" fill="currentColor"></path>
                        </svg>
                    )}
                </h3>
                <div className="flex items-center gap-1">
                    <button 
                        onClick={handleEditClick}
                        className="p-1.5 rounded-md text-muted-neutral transition-all z-10 opacity-0 group-hover:opacity-100 hover:text-primary hover:bg-wardian-light/10"
                        title="Edit Item"
                    >
                        <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M15.232 5.232l3.536 3.536m-2.036-5.036a2.5 2.5 0 113.536 3.536L6.5 21.036H3v-3.572L16.732 3.732z" />
                        </svg>
                    </button>
                    <button 
                        onClick={(e) => { e.stopPropagation(); onToggleStar(e); }}
                        className={`p-1.5 rounded-md transition-colors z-10 ${item.metadata.is_starred ? 'text-yellow-400 hover:text-yellow-300' : 'text-muted-neutral opacity-0 group-hover:opacity-100 hover:text-primary'}`}
                        aria-label={`Toggle star for ${item.name}`}
                        title={`Toggle star for ${item.name}`}
                    >
                        <svg className="w-5 h-5" fill={item.metadata.is_starred ? "currentColor" : "none"} stroke="currentColor" viewBox="0 0 24 24">
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M11.049 2.927c.3-.921 1.603-.921 1.902 0l1.519 4.674a1 1 0 00.95.69h4.915c.969 0 1.371 1.24.588 1.81l-3.976 2.888a1 1 0 00-.363 1.118l1.518 4.674c.3.922-.755 1.688-1.538 1.118l-3.976-2.888a1 1 0 00-1.176 0l-3.976 2.888c-.783.57-1.838-.197-1.538-1.118l1.518-4.674a1 1 0 00-.363-1.118l-3.976-2.888c-.784-.57-.38-1.81.588-1.81h4.914a1 1 0 00.951-.69l1.519-4.674z" />
                        </svg>
                    </button>
                </div>
            </div>
            
            <p className="text-muted text-sm line-clamp-3 overflow-hidden flex-1 pointer-events-none">
                {item.content || "No content."}
            </p>

            {item.metadata.tags && item.metadata.tags.length > 0 && (
                <div className="flex flex-wrap gap-1 mt-2 pointer-events-none">
                    {item.metadata.tags.map(tag => (
                        <span key={tag} className="px-2 py-0.5 bg-wardian-light/10 text-[var(--color-wardian-accent)] border border-[var(--color-wardian-accent)]/20 rounded text-[10px] font-bold tracking-wide">
                            {tag}
                        </span>
                    ))}
                </div>
            )}
        </div>
    );
};
