import React, { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { LibraryPrompt, AgentConfig } from '../../types';
import { flattenPromptForInjection, submitInputToAgent } from '../../utils/terminalInput';

interface AssignPromptModalProps {
    prompt: LibraryPrompt;
    isOpen: boolean;
    onClose: () => void;
}

export const AssignPromptModal: React.FC<AssignPromptModalProps> = ({ prompt, isOpen, onClose }) => {
    const agentSelectId = React.useId();
    const [selectedTargetId, setSelectedTargetId] = useState<string>('');
    const [isInjecting, setIsInjecting] = useState(false);
    const [agents, setAgents] = useState<AgentConfig[]>([]);

    useEffect(() => {
        if (isOpen) {
            invoke<AgentConfig[]>('list_agents').then((fetchedAgents) => {
                setAgents(fetchedAgents);
                if (fetchedAgents.length > 0) {
                    setSelectedTargetId(fetchedAgents[0].session_id);
                } else {
                    setSelectedTargetId('');
                }
            }).catch(console.error);
        }
    }, [isOpen]);

    if (!isOpen) return null;

    const handleInject = async () => {
        if (!selectedTargetId) return;
        
        setIsInjecting(true);
        try {
            const flattenedPrompt = flattenPromptForInjection(prompt.content);
            await submitInputToAgent(selectedTargetId, flattenedPrompt);
            onClose();
        } catch (e) {
            console.error('Failed to run prompt:', e);
            alert(`Failed to run prompt: ${e}`);
        } finally {
            setIsInjecting(false);
        }
    };

    return (
        <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4">
            <div className="bg-wardian-bg border border-wardian-border rounded-xl w-full max-w-md flex flex-col shadow-2xl overflow-hidden animate-in fade-in zoom-in duration-200">
                <div className="flex justify-between items-center p-4 border-b border-wardian-border bg-wardian-sidebar-primary">
                    <h2 className="text-lg font-bold text-primary flex items-center gap-2">
                        <svg className="w-4 h-4 text-wardian-success" fill="currentColor" viewBox="0 0 24 24"><path d="M8 5v14l11-7z" /></svg>
                        Run {prompt.name}
                    </h2>
                    <button onClick={onClose} disabled={isInjecting} className="text-muted hover:text-primary transition-colors disabled:opacity-50">
                        <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M6 18L18 6M6 6l12 12"></path></svg>
                    </button>
                </div>
                
                <div className="p-5 flex flex-col gap-4">
                    <div className="flex flex-col gap-2">
                        <label htmlFor={agentSelectId} className="text-xs font-bold text-muted tracking-wide">Select agent</label>
                        <select 
                            id={agentSelectId}
                            value={selectedTargetId}
                            onChange={(e) => setSelectedTargetId(e.target.value)}
                            disabled={isInjecting || agents.length === 0}
                            className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-primary text-sm focus:outline-none focus:border-[var(--color-wardian-accent)]"
                        >
                            {agents.length === 0 && <option value="">No active agents</option>}
                            {agents.map(a => (
                                <option key={a.session_id} value={a.session_id}>{a.session_name} ({a.agent_class})</option>
                            ))}
                        </select>
                    </div>
                </div>

                <div className="p-4 border-t border-wardian-border bg-wardian-sidebar-primary flex justify-end gap-3">
                    <button 
                        onClick={onClose} 
                        disabled={isInjecting}
                        className="px-4 py-2 rounded text-sm font-bold text-muted hover:text-primary transition-colors disabled:opacity-50"
                    >
                        Cancel
                    </button>
                    <button 
                        onClick={handleInject} 
                        disabled={!selectedTargetId || isInjecting || agents.length === 0}
                        className="px-4 py-2 rounded flex items-center justify-center min-w-[100px] text-sm font-bold bg-[var(--color-wardian-accent)] text-[var(--color-wardian-bg)] hover:brightness-110 transition-all shadow-[0_0_10px_var(--color-wardian-accent)] disabled:opacity-50 disabled:shadow-none"
                    >
                        {isInjecting ? (
                            <div className="animate-spin w-4 h-4 border-2 border-[var(--color-wardian-bg)]/30 border-t-[var(--color-wardian-bg)] rounded-full"></div>
                        ) : "Run Prompt"}
                    </button>
                </div>
            </div>
        </div>
    );
};
