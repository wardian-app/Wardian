import React from 'react';
import { LibraryPrompt } from '../../types';

interface LibraryCardProps {
    prompt: LibraryPrompt;
    onClick: () => void;
    onToggleStar: (e: React.MouseEvent) => void;
}

export const LibraryCard: React.FC<LibraryCardProps> = ({ prompt, onClick, onToggleStar }) => {
    return (
        <div 
            className="bg-wardian-card-bg border border-wardian-border rounded-xl p-4 flex flex-col gap-3 cursor-pointer hover:border-[var(--color-wardian-accent)] transition-all group"
            onClick={onClick}
        >
            <div className="flex justify-between items-start">
                <h3 className="text-primary font-bold text-lg truncate" title={prompt.name}>
                    {prompt.name}
                </h3>
                <button 
                    onClick={onToggleStar}
                    className={`p-1.5 rounded-md transition-colors ${prompt.metadata.is_starred ? 'text-yellow-400 hover:text-yellow-300' : 'text-muted-neutral hover:text-primary'}`}
                >
                    <svg className="w-5 h-5" fill={prompt.metadata.is_starred ? "currentColor" : "none"} stroke="currentColor" viewBox="0 0 24 24">
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M11.049 2.927c.3-.921 1.603-.921 1.902 0l1.519 4.674a1 1 0 00.95.69h4.915c.969 0 1.371 1.24.588 1.81l-3.976 2.888a1 1 0 00-.363 1.118l1.518 4.674c.3.922-.755 1.688-1.538 1.118l-3.976-2.888a1 1 0 00-1.176 0l-3.976 2.888c-.783.57-1.838-.197-1.538-1.118l1.518-4.674a1 1 0 00-.363-1.118l-3.976-2.888c-.784-.57-.38-1.81.588-1.81h4.914a1 1 0 00.951-.69l1.519-4.674z" />
                    </svg>
                </button>
            </div>
            
            <p className="text-muted text-sm line-clamp-3 overflow-hidden flex-1">
                {prompt.content || "No content."}
            </p>

            {prompt.metadata.tags && prompt.metadata.tags.length > 0 && (
                <div className="flex flex-wrap gap-1 mt-2">
                    {prompt.metadata.tags.map(tag => (
                        <span key={tag} className="px-2 py-0.5 bg-wardian-light/10 text-[var(--color-wardian-accent)] border border-[var(--color-wardian-accent)]/20 rounded text-[10px] uppercase font-bold tracking-wider">
                            {tag}
                        </span>
                    ))}
                </div>
            )}
        </div>
    );
};
