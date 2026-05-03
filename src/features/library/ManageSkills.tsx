import React, { useCallback, useEffect, useRef, useState } from 'react';
import { useLibraryStore } from '../../store/useLibraryStore';
import { DeployedSkillRef, LibraryFolder, LibrarySkill } from '../../types';

interface ManageSkillsProps {
    targetType: 'agent' | 'class';
    targetId: string;
}

export const ManageSkills: React.FC<ManageSkillsProps> = ({ targetType, targetId }) => {
    const { skillTree, listDeployedSkillRefs, deploySkill, removeDeployedSkill, subscribeToLibraryChanges } = useLibraryStore();
    const [deployedSkills, setDeployedSkills] = useState<DeployedSkillRef[]>([]);
    const [availableSkills, setAvailableSkills] = useState<LibrarySkill[]>([]);
    const [isLoading, setIsLoading] = useState(false);
    const [selectedSkillToDeploy, setSelectedSkillToDeploy] = useState<string>('');
    const refreshRequestIdRef = useRef(0);
    const mountedRef = useRef(false);

    const refreshSkills = useCallback(async () => {
        const requestId = refreshRequestIdRef.current + 1;
        refreshRequestIdRef.current = requestId;
        const requestTargetType = targetType;
        const requestTargetId = targetId;
        setIsLoading(true);
        try {
            const list = await listDeployedSkillRefs(requestTargetType, requestTargetId);
            if (
                mountedRef.current &&
                refreshRequestIdRef.current === requestId &&
                requestTargetType === targetType &&
                requestTargetId === targetId
            ) {
                setDeployedSkills(list);
            }
        } catch (e) {
            console.error('Failed to fetch deployed skills', e);
        } finally {
            if (mountedRef.current && refreshRequestIdRef.current === requestId) {
                setIsLoading(false);
            }
        }
    }, [listDeployedSkillRefs, targetId, targetType]);

    useEffect(() => {
        mountedRef.current = true;
        return () => {
            mountedRef.current = false;
            refreshRequestIdRef.current += 1;
        };
    }, []);

    useEffect(() => subscribeToLibraryChanges('skills'), [subscribeToLibraryChanges]);

    useEffect(() => {
        if (!skillTree) {
            setAvailableSkills([]);
            void refreshSkills();
            return;
        }

        const skills: LibrarySkill[] = [];
        function traverse(folder: LibraryFolder) {
            for (const child of folder.children) {
                if ('description' in child) {
                    skills.push(child as LibrarySkill);
                } else if ('children' in child) {
                    traverse(child as LibraryFolder);
                }
            }
        }
        traverse(skillTree);
        setAvailableSkills(skills);
        void refreshSkills();
    }, [refreshSkills, skillTree]);

    const handleDeploy = async () => {
        if (!selectedSkillToDeploy) return;
        setIsLoading(true);
        try {
            const skill = availableSkills.find(s => s.path === selectedSkillToDeploy);
            if (skill) {
                await deploySkill(skill.path, targetType, targetId);
                await refreshSkills();
                setSelectedSkillToDeploy('');
            }
        } catch (e) {
            console.error('Failed to deploy skill', e);
            alert(`Failed to deploy skill: ${e}`);
        } finally {
            setIsLoading(false);
        }
    };

    const handleRemove = async (skillName: string) => {
        setIsLoading(true);
        try {
            await removeDeployedSkill(targetType, targetId, skillName);
            await refreshSkills();
        } catch (e) {
            console.error('Failed to remove skill', e);
            alert(`Failed to remove skill: ${e}`);
        } finally {
            setIsLoading(false);
        }
    };

    return (
        <div className="flex flex-col gap-3 mt-4">
            <h3 className="text-xs font-bold text-muted tracking-wide">Manage Skills</h3>
            
            <div className="flex flex-col gap-2 bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded-lg p-3">
                {deployedSkills.length === 0 ? (
                    <p className="text-[10px] text-muted-neutral italic">No skills currently deployed.</p>
                ) : (
                    deployedSkills.map(skill => (
                        <div key={skill.source_path ?? skill.name} className="flex items-center justify-between group">
                            <span className="text-xs text-primary font-mono">{skill.source_path ?? skill.name}</span>
                            <button 
                                onClick={() => handleRemove(skill.name)}
                                disabled={isLoading}
                                className="text-muted hover:text-red-400 opacity-0 group-hover:opacity-100 transition-opacity"
                                title="Remove Skill"
                            >
                                <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M6 18L18 6M6 6l12 12"></path></svg>
                            </button>
                        </div>
                    ))
                )}
            </div>

            <div className="flex gap-2 items-center">
                <select 
                    value={selectedSkillToDeploy}
                    onChange={(e) => setSelectedSkillToDeploy(e.target.value)}
                    disabled={isLoading}
                    className="flex-1 min-w-0 truncate bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-2 py-1.5 text-xs text-primary focus:outline-none focus:border-[var(--color-wardian-accent)]"
                >
                    <option value="">Select a skill to deploy...</option>
                    {availableSkills.filter(s => !deployedSkills.some((skill) => (
                        skill.source_path ? skill.source_path === s.path : skill.name === s.name
                    ))).map(s => (
                        <option key={s.path} value={s.path}>{s.name}{s.path !== s.name ? ` (${s.path})` : ''}</option>
                    ))}
                </select>
                <button 
                    onClick={handleDeploy}
                    disabled={!selectedSkillToDeploy || isLoading}
                    className={`px-3 py-1.5 rounded text-[10px] font-bold tracking-wide transition-all flex items-center justify-center gap-2 ${
                        !selectedSkillToDeploy || isLoading ? "bg-wardian-border text-muted cursor-not-allowed" : 
                        "bg-wardian-card-bg-muted border border-wardian-light/50 text-primary hover:border-[var(--color-wardian-accent)] hover:text-[var(--color-wardian-accent)]"
                    }`}
                >
                    {isLoading ? (
                        <div className="animate-spin w-3 h-3 border-2 border-primary/30 border-t-primary rounded-full"></div>
                    ) : "Deploy"}
                </button>
            </div>
        </div>
    );
};
