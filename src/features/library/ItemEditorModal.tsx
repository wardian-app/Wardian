import React, { useState, useEffect } from 'react';
import { LibraryPrompt, LibrarySkill, LibraryItemMetadata } from '../../types';

interface ItemEditorModalProps {
    item: LibraryPrompt | LibrarySkill;
    isOpen: boolean;
    onClose: () => void;
    onSave: (path: string, content: string, metadata: LibraryItemMetadata) => void;
}

export const ItemEditorModal: React.FC<ItemEditorModalProps> = ({ item, isOpen, onClose, onSave }) => {
    const [content, setContent] = useState(item.content);
    const [tagsText, setTagsText] = useState(item.metadata.tags.join(', '));
    const [isStarred, setIsStarred] = useState(item.metadata.is_starred);

    useEffect(() => {
        if (isOpen) {
            setContent(item.content);
            setTagsText(item.metadata.tags.join(', '));
            setIsStarred(item.metadata.is_starred);
        }
    }, [isOpen, item]);

    if (!isOpen) return null;

    const handleSave = () => {
        const updatedMetadata = {
            ...item.metadata,
            tags: tagsText.split(',').map(t => t.trim()).filter(t => t.length > 0),
            is_starred: isStarred
        };
        onSave(item.path, content, updatedMetadata);
        onClose();
    };

    return (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm p-4">
            <div className="bg-wardian-bg border border-wardian-border rounded-xl w-full max-w-3xl h-[80vh] flex flex-col shadow-2xl overflow-hidden animate-in fade-in zoom-in duration-200">
                <div className="flex justify-between items-center p-4 border-b border-wardian-border bg-wardian-sidebar-primary">
                    <div className="flex items-center gap-2">
                        <button 
                            onClick={() => setIsStarred(!isStarred)}
                            className={`transition-colors ${isStarred ? 'text-yellow-400 hover:text-yellow-300' : 'text-muted-neutral hover:text-primary'}`}
                        >
                            <svg className="w-5 h-5" fill={isStarred ? "currentColor" : "none"} stroke="currentColor" viewBox="0 0 24 24">
                                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M11.049 2.927c.3-.921 1.603-.921 1.902 0l1.519 4.674a1 1 0 00.95.69h4.915c.969 0 1.371 1.24.588 1.81l-3.976 2.888a1 1 0 00-.363 1.118l1.518 4.674c.3.922-.755 1.688-1.538 1.118l-3.976-2.888a1 1 0 00-1.176 0l-3.976 2.888c-.783.57-1.838-.197-1.538-1.118l1.518-4.674a1 1 0 00-.363-1.118l-3.976-2.888c-.784-.57-.38-1.81.588-1.81h4.914a1 1 0 00.951-.69l1.519-4.674z" />
                            </svg>
                        </button>
                        <h2 className="text-xl font-bold text-primary">{item.name}</h2>
                    </div>
                    <button onClick={onClose} className="text-muted hover:text-primary transition-colors">
                        <svg className="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M6 18L18 6M6 6l12 12"></path></svg>
                    </button>
                </div>
                
                <div className="flex-1 overflow-y-auto p-4 flex flex-col gap-4">
                    <div className="flex flex-col gap-1">
                        <label className="text-xs font-bold text-muted uppercase tracking-widest">Content</label>
                        <textarea
                            className="w-full h-64 bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded p-3 text-primary text-sm focus:outline-none focus:border-[var(--color-wardian-accent)] font-mono resize-y"
                            value={content}
                            onChange={e => setContent(e.target.value)}
                        />
                    </div>
                    
                    <div className="flex flex-col gap-1">
                        <label className="text-xs font-bold text-muted uppercase tracking-widest">Tags (comma separated)</label>
                        <input
                            type="text"
                            className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-primary text-sm focus:outline-none focus:border-[var(--color-wardian-accent)]"
                            value={tagsText}
                            onChange={e => setTagsText(e.target.value)}
                            placeholder="e.g. react, debug, snippet"
                        />
                    </div>
                </div>

                <div className="p-4 border-t border-wardian-border bg-wardian-sidebar-primary flex justify-end gap-3">
                    <button onClick={onClose} className="px-4 py-2 rounded text-sm font-bold text-muted hover:text-primary transition-colors">
                        Cancel
                    </button>
                    <button onClick={handleSave} className="px-4 py-2 rounded text-sm font-bold bg-[var(--color-wardian-accent)] text-[var(--color-wardian-bg)] hover:brightness-110 transition-all shadow-[0_0_10px_var(--color-wardian-accent)]">
                        Save Changes
                    </button>
                </div>
            </div>
        </div>
    );
};
