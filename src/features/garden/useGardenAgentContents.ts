import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AgentConfig } from "../../types";

/** Wire models mirror wardian_core::memory; scope belongs to each record. */
export interface GardenMemorySource {
  source_type: string;
  locator?: string;
  source_hash?: string;
  primary: boolean;
}

export interface GardenMemoryRecord {
  revision_id: string;
  memory_id: string;
  revision: number;
  agent_id: string;
  workspace: string | null;
  kind: "stable" | "current";
  text: string;
  evidence_excerpt: string;
  evidence_hash: string;
  status: "active" | "superseded" | "removed";
  supersedes_revision_id: string | null;
  replaced_by_revision_id: string | null;
  created_at: string;
  updated_at: string;
  last_verified_at: string;
  idempotency_key: string | null;
  sources: GardenMemorySource[];
}

/** Canonical archive index, not the live terminal buffer or an execution queue. */
export interface GardenConversationEntry {
  schema: number;
  conversation_id: string;
  agent_id: string;
  agent_name: string;
  agent_class: string;
  workspace: string;
  provider: string;
  provider_session_ids: string[];
  started_at: string;
  ended_at: string | null;
  status: "open" | "closed" | "interrupted";
  boundary_reason: "spawn" | "provider_source_changed" | "clear" | "worktree_switch" | "logging_enabled" | "shutdown";
  first_prompt_excerpt: string | null;
  last_record_excerpt: string | null;
  record_count: number;
  turn_count: number;
  has_turns: boolean;
  lifecycle_only: boolean;
  artifact_count: number;
  path: string;
}

export interface GardenContentState<T> {
  data: T | null;
  loading: boolean;
  error: string | null;
  /** A previous successful snapshot is shown during refresh or after its failure. */
  stale: boolean;
}

/** Fetch a canonical record for the parent-owned Record plane. */
export function readGardenMemory(memoryId: string): Promise<GardenMemoryRecord> {
  return invoke("memory_get", { memoryId });
}

/** Fetch revisions only when a Record plane asks for history. */
export function readGardenMemoryHistory(memoryId: string): Promise<GardenMemoryRecord[]> {
  return invoke("memory_history", { memoryId });
}

const CACHE_TTL = 30_000;
const CACHE_LIMIT = 128;

interface CacheEntry<T> {
  value: GardenContentState<T>;
  updatedAt: number;
  pending?: Promise<GardenContentState<T>>;
}

/** Bounded caches owned by one Garden view; never shared across application sessions. */
export interface GardenContentsCache {
  memories: Map<string, CacheEntry<GardenMemoryRecord[]>>;
  conversations: Map<string, CacheEntry<GardenConversationEntry[]>>;
}

/** Allocate once per Garden view and pass to its canonical readers. */
export function createGardenContentsCache(): GardenContentsCache {
  return { memories: new Map(), conversations: new Map() };
}

function emptyState<T>(loading = true): GardenContentState<T> {
  return { data: null, loading, error: null, stale: false };
}

function useCanonicalRead<T>(key: string, read: () => Promise<T>, revision: number, enabled: boolean,
  cache: Map<string, CacheEntry<T>>): GardenContentState<T> {
  const [snapshot, setSnapshot] = useState<{ key: string; value: GardenContentState<T> }>(() => ({ key, value: emptyState<T>() }));
  const consumedRevision = useRef(0);
  useEffect(() => {
    if (!enabled) return;
    let active = true;
    const refresh = async (force = false) => {
      let entry = cache.get(key);
      if (entry) {
        cache.delete(key);
        cache.set(key, entry);
      } else {
        entry = { value: emptyState<T>(), updatedAt: 0 };
        cache.set(key, entry);
        if (cache.size > CACHE_LIMIT) {
          const oldest = cache.keys().next().value;
          if (oldest !== undefined) cache.delete(oldest);
        }
      }
      if (!force && !entry.pending && entry.value.data !== null && !entry.value.stale && Date.now() - entry.updatedAt < CACHE_TTL) {
        setSnapshot({ key, value: entry.value });
        return;
      }
      if (!entry.pending) {
        const target = entry;
        target.value = { ...target.value, loading: true, error: null, stale: target.value.data !== null };
        target.pending = Promise.resolve().then(read).then((data) => {
          target.updatedAt = Date.now();
          target.value = { data, loading: false, error: null, stale: false };
          return target.value;
        }, (error: unknown) => {
          target.value = { ...target.value, loading: false, error: String(error), stale: target.value.data !== null };
          return target.value;
        }).finally(() => { target.pending = undefined; });
      }
      setSnapshot({ key, value: entry.value });
      const value = await entry.pending ?? entry.value;
      if (active) setSnapshot({ key, value });
    };
    const force = consumedRevision.current !== revision;
    consumedRevision.current = revision;
    void refresh(force);
    const timer = window.setInterval(() => { void refresh(); }, CACHE_TTL);
    return () => { active = false; window.clearInterval(timer); };
  }, [key, read, revision, enabled, cache]);
  // Never flash the previous agent's private contents before effects run.
  const value = snapshot.key === key ? snapshot.value : emptyState<T>(enabled);
  return enabled ? value : { ...value, loading: false };
}

/** Lazy cutaway contents. Does not change Library, Inbox, or Workbench selection. */
export function useGardenAgentContents(agent: AgentConfig, enabled = true, cache?: GardenContentsCache) {
  const agentId = agent.session_id;
  const workspace = agent.git_worktree_folder || agent.folder;
  const [revision, setRevision] = useState(0);
  const [localCache] = useState(createGardenContentsCache);
  const contentsCache = cache ?? localCache;
  const readMemories = useCallback(() => invoke<GardenMemoryRecord[]>("memory_list", {
    agentId, workspace: workspace || null,
  }), [agentId, workspace]);
  const readConversations = useCallback(async () => {
    const result = await invoke<{ schema: number; conversations: GardenConversationEntry[] }>("list_conversations", {
      agent: agentId, scopeAll: false,
    });
    return result.conversations.filter((entry) => entry.agent_id === agentId)
      .sort((left, right) => right.started_at.localeCompare(left.started_at));
  }, [agentId]);
  const memories = useCanonicalRead(JSON.stringify([agentId, workspace || null]), readMemories, revision, enabled, contentsCache.memories);
  // The archive command is agent-scoped, independent of the selected workspace.
  const conversations = useCanonicalRead(agentId, readConversations, revision, enabled, contentsCache.conversations);
  const refresh = useCallback(() => setRevision((value) => value + 1), []);
  return { memories, conversations, refresh };
}
