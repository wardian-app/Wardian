import React, { useState } from 'react';
import { LibraryPrompt, LibrarySkill } from '../../types';

interface LibraryCardProps {
    item: LibraryPrompt | LibrarySkill;
    hasSelectedAgents: boolean;
    onClick: () => void;
    onToggleStar: (e: React.MouseEvent) => void;
    onAction: (e: React.MouseEvent) => Promise<void> | void;
}

export const LibraryCard: React.FC<LibraryCardProps> = ({ item, hasSelectedAgents, onClick, onToggleStar, onAction }) => {
    const isSkill = 'description' in item;
    const [isActioning, setIsActioning] = useState(false);

    const handleAction = async (e: React.MouseEvent) => {
        e.stopPropagation();
        setIsActioning(true);
        try {
            await onAction(e);
        } finally {
            setIsActioning(false);
        }
    };

    return (
        <div 
            className="bg-wardian-card-bg border border-wardian-border rounded-xl p-4 flex flex-col gap-3 cursor-pointer hover:border-[var(--color-wardian-accent)] transition-all group relative"
            onClick={onClick}
        >
            <div className="flex justify-between items-start">
                <h3 className="text-primary font-bold text-lg truncate flex items-center gap-2" title={item.name}>
                    {item.name}
                </h3>
                <div className="flex items-center gap-1">
                    {hasSelectedAgents && (
                        <button 
                            onClick={handleAction}
                            disabled={isActioning}
                            className={`p-1.5 rounded-md text-wardian-success transition-all z-10 ${isActioning ? 'opacity-100' : 'opacity-0 group-hover:opacity-100 hover:bg-wardian-success/10'}`}
                            title={isSkill ? "Deploy to Selected Agent(s)" : "Send to Selected Agent(s)"}
                        >
                            {isActioning ? (
                                <svg className="w-5 h-5 animate-spin" viewBox="0 0 24 24" fill="none" stroke="currentColor">
                                    <circle cx="12" cy="12" r="10" strokeWidth="4" strokeOpacity="0.25"></circle>
                                    <path d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" fill="currentColor"></path>
                                </svg>
                            ) : (
                                <svg className="w-5 h-5" fill="currentColor" viewBox="0 0 24 24">
                                    {isSkill ? (
                                        <path d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-8l-4-4m0 0L8 8m4-4v12" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" fill="none"/>
                                    ) : (
                                        <path d="M8 5v14l11-7z" />
                                    )}
                                </svg>
                            )}
                        </button>
                    )}
                    <button 
                        onClick={(e) => { e.stopPropagation(); onToggleStar(e); }}
                        className={`p-1.5 rounded-md transition-colors z-10 ${item.metadata.is_starred ? 'text-yellow-400 hover:text-yellow-300' : 'text-muted-neutral opacity-0 group-hover:opacity-100 hover:text-primary'}`}
                    >
                        <svg className="w-5 h-5" fill={item.metadata.is_starred ? "currentColor" : "none"} stroke="currentColor" viewBox="0 0 24 24">
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M11.049 2.927c.3-.921 1.603-.921 1.902 0l1.519 4.674a1 1 0 00.95.69h4.915c.969 0 1.371 1.24.588 1.81l-3.976 2.888a1 1 0 00-.363 1.118l1.518 4.674c.3.922-.755 1.688-1.538 1.118l-3.976-2.888a1 1 0 00-1.176 0l-3.976 2.888c-.783.57-1.838-.197-1.538-1.118l1.518-4.674a1 1 0 00-.363-1.118l-3.976-2.888c-.784-.57-.38-1.81.588-1.81h4.914a1 1 0 00.951-.69l1.519-4.674z" />
                        </svg>
                    </button>
                </div>
            </div>
            
            <p className="text-muted text-sm line-clamp-3 overflow-hidden flex-1">
                {item.content || "No content."}
            </p>

            {item.metadata.tags && item.metadata.tags.length > 0 && (
                <div className="flex flex-wrap gap-1 mt-2">
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
