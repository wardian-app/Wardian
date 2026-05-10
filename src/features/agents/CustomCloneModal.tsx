import React, { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  AgentClassDefinition,
  AgentClonePreview,
  CloneFileTreeNode,
  DeployedSkillRef,
} from "../../types";

interface CustomCloneModalProps {
  sourceSessionId: string;
  agentClasses: AgentClassDefinition[];
  isOpen: boolean;
  onClose: () => void;
  onCloned: () => void;
}

const providerOptions = [
  { value: "claude", label: "Claude" },
  { value: "codex", label: "Codex" },
  { value: "gemini", label: "Gemini" },
  { value: "opencode", label: "OpenCode" },
];

const skillKey = (skill: DeployedSkillRef) => `${skill.name}\u0000${skill.source_path ?? ""}`;

const descendantFilePaths = (node: CloneFileTreeNode): string[] => {
  if (node.kind === "file") return [node.path];
  return node.children.flatMap(descendantFilePaths);
};

const testIdSuffix = (value: string) => value.replace(/[^a-zA-Z0-9_-]/g, "-");

const errorMessage = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

export const CustomCloneModal: React.FC<CustomCloneModalProps> = ({
  sourceSessionId,
  agentClasses,
  isOpen,
  onClose,
  onCloned,
}) => {
  const [preview, setPreview] = useState<AgentClonePreview | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [cloneName, setCloneName] = useState("");
  const [provider, setProvider] = useState("claude");
  const [agentClass, setAgentClass] = useState("");
  const [folder, setFolder] = useState("");
  const [selectedFiles, setSelectedFiles] = useState<Set<string>>(new Set());
  const [selectedSkills, setSelectedSkills] = useState<Set<string>>(new Set());

  useEffect(() => {
    if (!isOpen || !sourceSessionId) return;

    let cancelled = false;
    setIsLoading(true);
    setError(null);
    setPreview(null);

    invoke<AgentClonePreview>("get_agent_clone_preview", { sourceSessionId })
      .then((nextPreview) => {
        if (cancelled) return;
        setPreview(nextPreview);
        setCloneName(nextPreview.suggested_session_name);
        setProvider(nextPreview.provider || "claude");
        setAgentClass(nextPreview.agent_class);
        setFolder(nextPreview.folder);
        setSelectedFiles(new Set(nextPreview.default_selected_files));
        setSelectedSkills(new Set(nextPreview.default_selected_skills.map(skillKey)));
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(errorMessage(err));
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [isOpen, sourceSessionId]);

  const selectedSkillRefs = useMemo(() => {
    if (!preview) return [];
    return preview.skills.filter((skill) => selectedSkills.has(skillKey(skill)));
  }, [preview, selectedSkills]);

  if (!isOpen) return null;

  const toggleFile = (path: string) => {
    setSelectedFiles((current) => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  const toggleDirectory = (node: CloneFileTreeNode) => {
    const paths = descendantFilePaths(node);
    setSelectedFiles((current) => {
      const next = new Set(current);
      const allSelected = paths.every((path) => next.has(path));
      for (const path of paths) {
        if (allSelected) next.delete(path);
        else next.add(path);
      }
      return next;
    });
  };

  const toggleSkill = (skill: DeployedSkillRef) => {
    const key = skillKey(skill);
    setSelectedSkills((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!preview || isSubmitting) return;
    if (!cloneName.trim()) {
      setError("Clone name is required.");
      return;
    }

    setIsSubmitting(true);
    setError(null);
    try {
      await invoke("clone_agent", {
        req: {
          source_session_id: preview.source_session_id,
          mode: "profile",
          session_name: cloneName.trim(),
          provider,
          agent_class: agentClass,
          folder,
          profile_selection: {
            files: Array.from(selectedFiles).sort(),
            skills: selectedSkillRefs,
          },
        },
      });
      onCloned();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setIsSubmitting(false);
    }
  };

  const renderFileNode = (node: CloneFileTreeNode, depth = 0): React.ReactNode => {
    if (node.kind === "directory") {
      const descendants = descendantFilePaths(node);
      const checked = descendants.length > 0 && descendants.every((path) => selectedFiles.has(path));
      return (
        <div key={node.path || "__root"}>
          {node.path && (
            <label
              className="flex items-center gap-2 px-2 py-1 text-xs text-muted-neutral"
              style={{ paddingLeft: `${8 + depth * 16}px` }}
            >
              <input
                type="checkbox"
                checked={checked}
                onChange={() => toggleDirectory(node)}
                className="accent-[var(--color-wardian-accent)]"
                aria-label={node.path}
                data-testid={`custom-clone-file-${testIdSuffix(node.path)}`}
              />
              <span>{node.name}</span>
            </label>
          )}
          {node.children.map((child) => renderFileNode(child, node.path ? depth + 1 : depth))}
        </div>
      );
    }

    return (
      <label
        key={node.path}
        className="flex items-center gap-2 px-2 py-1 text-xs text-primary"
        style={{ paddingLeft: `${8 + depth * 16}px` }}
      >
        <input
          type="checkbox"
          checked={selectedFiles.has(node.path)}
          onChange={() => toggleFile(node.path)}
          className="accent-[var(--color-wardian-accent)]"
          aria-label={node.path}
          data-testid={`custom-clone-file-${testIdSuffix(node.path)}`}
        />
        <span className="font-mono">{node.path}</span>
      </label>
    );
  };

  return (
    <div className="fixed inset-0 z-[10000] flex items-center justify-center bg-black/55 px-4">
      <form
        role="dialog"
        aria-label="Custom Clone"
        data-testid="custom-clone-modal"
        onSubmit={submit}
        className="flex max-h-[90vh] w-full max-w-3xl flex-col overflow-hidden rounded-lg border border-wardian-border bg-[var(--color-wardian-card)] shadow-2xl"
      >
        <div className="flex items-center justify-between border-b border-wardian-border px-4 py-3">
          <h2 className="text-sm font-bold text-primary">Custom Clone</h2>
          <button type="button" onClick={onClose} className="text-sm text-muted-neutral hover:text-primary">
            Close
          </button>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto p-4">
          {isLoading && <div className="text-sm text-muted-neutral">Loading...</div>}
          {error && (
            <div className="mb-3 rounded border border-wardian-error/40 bg-wardian-error/10 px-3 py-2 text-sm text-wardian-error">
              {error}
            </div>
          )}
          {preview && (
            <div className="grid gap-4 md:grid-cols-[1fr_1fr]">
              <div className="space-y-3">
                <label className="block text-[10px] font-bold text-muted-neutral">
                  Clone Name
                  <input
                    aria-label="Clone Name"
                    value={cloneName}
                    onChange={(event) => setCloneName(event.target.value)}
                    className="mt-1 w-full rounded border border-wardian-border bg-[var(--color-wardian-input-bg)] px-3 py-2 text-sm text-primary focus:outline-none"
                  />
                </label>
                <label className="block text-[10px] font-bold text-muted-neutral">
                  Provider Engine
                  <select
                    aria-label="Provider Engine"
                    data-testid="custom-clone-provider"
                    value={provider}
                    onChange={(event) => setProvider(event.target.value)}
                    className="mt-1 w-full rounded border border-wardian-border bg-[var(--color-wardian-input-bg)] px-3 py-2 text-sm text-primary focus:outline-none"
                  >
                    {providerOptions.map((option) => (
                      <option key={option.value} value={option.value}>
                        {option.label}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="block text-[10px] font-bold text-muted-neutral">
                  Agent Class
                  <select
                    aria-label="Agent Class"
                    value={agentClass}
                    onChange={(event) => setAgentClass(event.target.value)}
                    className="mt-1 w-full rounded border border-wardian-border bg-[var(--color-wardian-input-bg)] px-3 py-2 text-sm text-primary focus:outline-none"
                  >
                    {agentClasses.map((agentClassOption) => (
                      <option key={agentClassOption.name} value={agentClassOption.name}>
                        {agentClassOption.name}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="block text-[10px] font-bold text-muted-neutral">
                  Workspace Path
                  <input
                    aria-label="Workspace Path"
                    value={folder}
                    onChange={(event) => setFolder(event.target.value)}
                    className="mt-1 w-full rounded border border-wardian-border bg-[var(--color-wardian-input-bg)] px-3 py-2 font-mono text-xs text-primary focus:outline-none"
                  />
                </label>
              </div>

              <div className="space-y-3">
                <div>
                  <h3 className="mb-1 text-[10px] font-bold text-muted-neutral">Files</h3>
                  <div className="max-h-52 overflow-auto rounded border border-wardian-border bg-[var(--color-wardian-input-bg)] py-1">
                    {preview.files.children.length === 0 ? (
                      <div className="px-2 py-2 text-xs text-muted-neutral">No eligible files</div>
                    ) : (
                      preview.files.children.map((child) => renderFileNode(child))
                    )}
                  </div>
                </div>
                <div>
                  <h3 className="mb-1 text-[10px] font-bold text-muted-neutral">Skills</h3>
                  <div className="max-h-40 overflow-auto rounded border border-wardian-border bg-[var(--color-wardian-input-bg)] py-1">
                    {preview.skills.length === 0 ? (
                      <div className="px-2 py-2 text-xs text-muted-neutral">No agent skills</div>
                    ) : (
                      preview.skills.map((skill) => (
                        <label key={skillKey(skill)} className="flex items-center gap-2 px-2 py-1 text-xs text-primary">
                          <input
                            type="checkbox"
                            checked={selectedSkills.has(skillKey(skill))}
                            onChange={() => toggleSkill(skill)}
                            className="accent-[var(--color-wardian-accent)]"
                            aria-label={`${skill.name} ${skill.source_path ?? ""}`.trim()}
                            data-testid={`custom-clone-skill-${testIdSuffix(skill.source_path ?? skill.name)}`}
                          />
                          <span>{skill.name}</span>
                          {skill.source_path && <span className="font-mono text-muted-neutral">{skill.source_path}</span>}
                        </label>
                      ))
                    )}
                  </div>
                </div>
              </div>
            </div>
          )}
        </div>

        <div className="flex justify-end gap-2 border-t border-wardian-border px-4 py-3">
          <button type="button" onClick={onClose} className="rounded border border-wardian-border px-3 py-2 text-xs text-muted-neutral hover:text-primary">
            Cancel
          </button>
          <button
            type="submit"
            data-testid="custom-clone-submit"
            disabled={!preview || isLoading || isSubmitting}
            className="rounded bg-[var(--color-wardian-accent)] px-4 py-2 text-xs font-bold text-[var(--color-wardian-bg)] disabled:opacity-50"
          >
            {isSubmitting ? "Cloning..." : "Clone"}
          </button>
        </div>
      </form>
    </div>
  );
};
