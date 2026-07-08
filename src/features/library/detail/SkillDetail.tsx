import React from 'react';
import { DeploymentTarget, SkillDeployment } from '../../../types';
import { DetailPanelCommonProps } from '../DetailPane';
import { MarkdownEditor } from '../MarkdownEditor';
import { DeployTargetsControl } from '../DeployTargetsControl';

interface SkillDetailProps extends DetailPanelCommonProps {
    deployments: DeploymentTarget[];
    /** Settles once the store's deploy/undeploy attempt is done (success or
     * failure), so `DeployTargetsControl` can clear its own pending state
     * for the affected chip/row. Not expected to reject — see that
     * control's `onApply` doc comment. */
    onApplyDeployments: (targets: SkillDeployment[]) => Promise<void>;
}

/** Skill panel: SKILL.md editor + deploy-target chips/picker. */
export const SkillDetail: React.FC<SkillDetailProps> = ({
    entry,
    header,
    draft,
    dirty,
    stale,
    onChange,
    onSave,
    onReloadExternal,
    onKeepMine,
    deployments,
    onApplyDeployments,
}) => {
    return (
        <div data-testid="skill-detail" className="flex flex-col h-full min-h-0">
            {header}
            <div className="flex-1 min-h-0">
                <MarkdownEditor
                    value={draft}
                    onChange={onChange}
                    onSave={onSave}
                    dirty={dirty}
                    stale={stale}
                    onReloadExternal={onReloadExternal}
                    onKeepMine={onKeepMine}
                />
            </div>
            <div className="border-t border-wardian-border p-3">
                <DeployTargetsControl entry={entry} deployments={deployments} onApply={onApplyDeployments} />
            </div>
        </div>
    );
};
