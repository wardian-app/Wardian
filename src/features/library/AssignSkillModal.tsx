import React, { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { LibrarySkill, AgentConfig, AgentClassDefinition } from '../../types';
import { useLibraryStore } from '../../store/useLibraryStore';

interface AssignSkillModalProps {
    skill: LibrarySkill;
    isOpen: boolean;
    onClose: () => void;
}

export const AssignSkillModal: React.FC<AssignSkillModalProps> = ({ skill, isOpen, onClose }) => {
    const { deploySkill, listSkillDeployments, removeDeployedSkill } = useLibraryStore();
    const [targetType, setTargetType] = useState<'user' | 'class' | 'agent'>('user');
    const [selectedTargetId, setSelectedTargetId] = useState<string>('global');
    const [isDeploying, setIsDeploying] = useState(false);
    
    const [agents, setAgents] = useState<AgentConfig[]>([]);
    const [classes, setClasses] = useState<AgentClassDefinition[]>([]);
    const [deployments, setDeployments] = useState<{target_type: string; target_id: string}[]>([]);

    const refreshDeployments = async () => {
        try {
            const list = await listSkillDeployments(skill.name);
            setDeployments(list);
        } catch (e) {
            console.error('Failed to fetch deployments:', e);
        }
    };

    useEffect(() => {
        if (isOpen) {
            setTargetType('user');
            setSelectedTargetId('global');
            
            // Fetch agents and classes for the dropdowns
            invoke<AgentConfig[]>('list_agents').then(setAgents).catch(console.error);
            invoke<AgentClassDefinition[]>('list_agent_classes').then(setClasses).catch(console.error);
            refreshDeployments();
        }
    }, [isOpen, skill.name]);

    useEffect(() => {
        if (targetType === 'user') {
            setSelectedTargetId('global');
        } else if (targetType === 'class' && classes.length > 0) {
            setSelectedTargetId(classes[0].name);
        } else if (targetType === 'agent' && agents.length > 0) {
            setSelectedTargetId(agents[0].session_id);
        } else {
            setSelectedTargetId('');
        }
    }, [targetType, agents, classes]);

    if (!isOpen) return null;

    const handleDeploy = async () => {
        if (!selectedTargetId) return;
        
        setIsDeploying(true);
        try {
            await deploySkill(skill.path, targetType, selectedTargetId);
            await refreshDeployments();
        } catch (e) {
            console.error('Failed to deploy skill:', e);
            alert(`Failed to deploy skill: ${e}`);
        } finally {
            setIsDeploying(false);
        }
    };

    const handleRemove = async (t_type: string, t_id: string) => {
        setIsDeploying(true);
        try {
            await removeDeployedSkill(t_type as any, t_id, skill.name);
            await refreshDeployments();
        } catch (e) {
            console.error('Failed to remove skill:', e);
            alert(`Failed to remove skill: ${e}`);
        } finally {
            setIsDeploying(false);
        }
    };

    return (
        <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4">
            <div className="bg-wardian-bg border border-wardian-border rounded-xl w-full max-w-md flex flex-col shadow-2xl overflow-hidden animate-in fade-in zoom-in duration-200">
                <div className="flex justify-between items-center p-4 border-b border-wardian-border bg-wardian-sidebar-primary">
                    <h2 className="text-lg font-bold text-primary flex items-center gap-2">
                        <svg className="w-4 h-4 text-[var(--color-wardian-accent)]" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M19.428 15.428a2 2 0 00-1.022-.547l-2.387-.477a6 6 0 00-3.86.517l-.318.158a6 6 0 01-3.86.517L6.05 15.21a2 2 0 00-1.806.547M8 4h8l-1 1v5.172a2 2 0 00.586 1.414l5 5c1.26 1.26.367 3.414-1.415 3.414H4.828c-1.782 0-2.674-2.154-1.414-3.414l5-5A2 2 0 009 10.172V5L8 4z" />
                        </svg>
                        Manage {skill.name}
                    </h2>
                    <button onClick={onClose} disabled={isDeploying} className="text-muted hover:text-primary transition-colors disabled:opacity-50">
                        <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M6 18L18 6M6 6l12 12"></path></svg>
                    </button>
                </div>
                
                <div className="p-5 flex flex-col gap-6 max-h-[60vh] overflow-y-auto no-scrollbar">
                    
                    {/* Existing Deployments */}
                    <div className="flex flex-col gap-2">
                        <h3 className="text-xs font-bold text-muted uppercase tracking-widest">Active Deployments</h3>
                        <div className="flex flex-col gap-2 bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded-lg p-3">
                            {deployments.length === 0 ? (
                                <p className="text-[10px] text-muted-neutral italic">Skill is not deployed anywhere.</p>
                            ) : (
                                deployments.map((dep, idx) => (
                                    <div key={idx} className="flex items-center justify-between group">
                                        <div className="flex flex-col">
                                            <span className="text-xs text-primary font-bold capitalize">{dep.target_type === 'user' ? 'Global User Profile' : dep.target_type}</span>
                                            <span className="text-[10px] text-muted-neutral font-mono">{dep.target_type === 'agent' ? agents.find(a => a.session_id === dep.target_id)?.session_name || dep.target_id : dep.target_id}</span>
                                        </div>
                                        <button 
                                            onClick={() => handleRemove(dep.target_type, dep.target_id)}
                                            disabled={isDeploying}
                                            className="p-1.5 rounded-md text-muted hover:text-red-400 hover:bg-red-400/10 transition-all opacity-0 group-hover:opacity-100 disabled:opacity-50"
                                            title="Remove Deployment"
                                        >
                                            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"></path></svg>
                                        </button>
                                    </div>
                                ))
                            )}
                        </div>
                    </div>

                    {/* New Deployment */}
                    <div className="flex flex-col gap-3">
                        <h3 className="text-xs font-bold text-muted uppercase tracking-widest">New Deployment</h3>
                        <div className="flex flex-col gap-2">
                            <label className="text-xs font-bold text-muted-neutral">Target Scope</label>
                            <select 
                                value={targetType}
                                onChange={(e) => setTargetType(e.target.value as any)}
                                disabled={isDeploying}
                                className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-primary text-sm focus:outline-none focus:border-[var(--color-wardian-accent)]"
                            >
                                <option value="user">All Agents (Global User Profile)</option>
                                <option value="class">Specific Agent Class</option>
                                <option value="agent">Specific Active Agent</option>
                            </select>
                        </div>

                        {targetType === 'class' && (
                            <div className="flex flex-col gap-2 animate-in slide-in-from-top-2 duration-200">
                                <label className="text-xs font-bold text-muted-neutral">Select Class</label>
                                <select 
                                    value={selectedTargetId}
                                    onChange={(e) => setSelectedTargetId(e.target.value)}
                                    disabled={isDeploying || classes.length === 0}
                                    className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-primary text-sm focus:outline-none focus:border-[var(--color-wardian-accent)]"
                                >
                                    {classes.length === 0 && <option value="">No custom classes available</option>}
                                    {classes.map(c => (
                                        <option key={c.name} value={c.name}>{c.name}</option>
                                    ))}
                                </select>
                            </div>
                        )}

                        {targetType === 'agent' && (
                            <div className="flex flex-col gap-2 animate-in slide-in-from-top-2 duration-200">
                                <label className="text-xs font-bold text-muted-neutral">Select Agent</label>
                                <select 
                                    value={selectedTargetId}
                                    onChange={(e) => setSelectedTargetId(e.target.value)}
                                    disabled={isDeploying || agents.length === 0}
                                    className="w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-3 py-2 text-primary text-sm focus:outline-none focus:border-[var(--color-wardian-accent)]"
                                >
                                    {agents.length === 0 && <option value="">No active agents</option>}
                                    {agents.map(a => (
                                        <option key={a.session_id} value={a.session_id}>{a.session_name} ({a.agent_class})</option>
                                    ))}
                                </select>
                            </div>
                        )}
                        
                        <button 
                            onClick={handleDeploy} 
                            disabled={!selectedTargetId || isDeploying}
                            className="mt-2 w-full px-4 py-2 rounded flex items-center justify-center text-sm font-bold bg-[var(--color-wardian-accent)] text-[var(--color-wardian-bg)] hover:brightness-110 transition-all shadow-[0_0_10px_var(--color-wardian-accent)] disabled:opacity-50 disabled:shadow-none"
                        >
                            {isDeploying ? (
                                <div className="animate-spin w-4 h-4 border-2 border-[var(--color-wardian-bg)]/30 border-t-[var(--color-wardian-bg)] rounded-full"></div>
                            ) : "Deploy Skill"}
                        </button>
                    </div>
                </div>
            </div>
        </div>
    );
};