import { useCallback, useEffect, useMemo, useRef, type RefObject } from "react";

import type { WorkbenchNodeV1 } from "../../types";
import type { WorkbenchIdKind, WorkbenchNavigationService } from "./navigationService";
import type { WorkbenchStore } from "./useWorkbenchStore";

export type WorkbenchCommandId =
  | "workbench.next_tab"
  | "workbench.previous_tab"
  | "workbench.next_group"
  | "workbench.previous_group"
  | "workbench.split_right"
  | "workbench.split_down"
  | "workbench.move_tab_next_group"
  | "workbench.move_tab_previous_group"
  | "workbench.close_surface"
  | "workbench.reopen_closed_surface"
  | "workbench.reset_workbench"
  | "workbench.toggle_group_zoom"
  | "workbench.open_surface"
  | "workbench.quick_open"
  | "workbench.command_palette"
  | "workbench.focus_left_dock"
  | "workbench.focus_right_dock"
  | "workbench.focus_workbench";

export type WorkbenchCommandAction = {
  command_id: WorkbenchCommandId;
  title: string;
  shortcut?: string;
};

export type WorkbenchCommandRouter = {
  actions: readonly WorkbenchCommandAction[];
  is_enabled: (commandId: WorkbenchCommandId) => boolean;
  execute: (commandId: WorkbenchCommandId) => Promise<boolean>;
};

export type UseWorkbenchCommandsOptions = {
  store: WorkbenchStore;
  navigation: WorkbenchNavigationService;
  root_ref: RefObject<HTMLElement | null>;
  enabled?: boolean;
  create_id?: (kind: WorkbenchIdKind) => string;
  on_quick_open?: () => void;
  on_command_palette?: () => void;
  on_focus_left_dock?: () => void;
  on_focus_right_dock?: () => void;
};

export const WORKBENCH_COMMAND_ACTIONS: readonly WorkbenchCommandAction[] = Object.freeze([
  { command_id: "workbench.open_surface", title: "Open Surface", shortcut: "Mod+Shift+O" },
  { command_id: "workbench.quick_open", title: "Quick Open", shortcut: "Mod+P" },
  { command_id: "workbench.command_palette", title: "Show Command Palette", shortcut: "Mod+Shift+P" },
  { command_id: "workbench.close_surface", title: "Close Surface", shortcut: "Mod+W" },
  { command_id: "workbench.reopen_closed_surface", title: "Reopen Closed Surface", shortcut: "Mod+Shift+T" },
  { command_id: "workbench.reset_workbench", title: "Reset Workbench" },
  { command_id: "workbench.next_tab", title: "Next Tab", shortcut: "Mod+]" },
  { command_id: "workbench.previous_tab", title: "Previous Tab", shortcut: "Mod+[" },
  { command_id: "workbench.next_group", title: "Focus Next Group", shortcut: "F6" },
  { command_id: "workbench.previous_group", title: "Focus Previous Group", shortcut: "Shift F6" },
  { command_id: "workbench.split_right", title: "Split Right", shortcut: "Mod+Alt+Right" },
  { command_id: "workbench.split_down", title: "Split Down", shortcut: "Mod+Alt+Down" },
  { command_id: "workbench.move_tab_next_group", title: "Move Tab to Next Group", shortcut: "Alt+Shift+Right" },
  { command_id: "workbench.move_tab_previous_group", title: "Move Tab to Previous Group", shortcut: "Alt+Shift+Left" },
  { command_id: "workbench.toggle_group_zoom", title: "Toggle Group Zoom", shortcut: "Alt+Shift+Z" },
  { command_id: "workbench.focus_left_dock", title: "Focus Left Dock", shortcut: "Mod+Alt+L" },
  { command_id: "workbench.focus_right_dock", title: "Focus Right Dock", shortcut: "Mod+Alt+R" },
  { command_id: "workbench.focus_workbench", title: "Focus Workbench", shortcut: "Mod+0" },
]);

function defaultCreateId(kind: WorkbenchIdKind): string {
  return `${kind}-${globalThis.crypto.randomUUID()}`;
}

function groupIdsInTreeOrder(node: WorkbenchNodeV1): string[] {
  return node.kind === "group"
    ? [node.group_id]
    : [...groupIdsInTreeOrder(node.first), ...groupIdsInTreeOrder(node.second)];
}

function wrappedIndex(index: number, delta: number, length: number): number {
  return ((index + delta) % length + length) % length;
}

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return target instanceof HTMLInputElement
    || target instanceof HTMLTextAreaElement
    || target instanceof HTMLSelectElement
    || target.isContentEditable;
}

function terminalOwnsShortcut(target: EventTarget | null): boolean {
  return target instanceof Element
    && target.closest('[data-terminal-shortcuts="terminal"]') !== null;
}

function focusSurface(root: HTMLElement | null, surfaceId: string): void {
  const tab = [...(root?.querySelectorAll<HTMLElement>('[role="tab"][data-surface-id]') ?? [])]
    .find((candidate) => candidate.dataset.surfaceId === surfaceId);
  tab?.focus();
}

function focusGroup(root: HTMLElement | null, groupId: string): void {
  const group = [...(root?.querySelectorAll<HTMLElement>('[data-testid="workbench-group"]') ?? [])]
    .find((candidate) => candidate.dataset.groupId === groupId);
  group?.focus();
}

function shortcutForEvent(event: KeyboardEvent): WorkbenchCommandId | null {
  const primary = event.ctrlKey || event.metaKey;
  const key = event.key.toLowerCase();
  if (event.key === "F6") {
    return event.shiftKey ? "workbench.previous_group" : "workbench.next_group";
  }
  if (primary && event.shiftKey && key === "p") return "workbench.command_palette";
  if (primary && !event.shiftKey && key === "p") return "workbench.quick_open";
  if (primary && event.shiftKey && key === "t") return "workbench.reopen_closed_surface";
  if (primary && event.shiftKey && key === "o") return "workbench.open_surface";
  if (primary && !event.shiftKey && key === "w") return "workbench.close_surface";
  if (primary && !event.shiftKey && event.key === "]") return "workbench.next_tab";
  if (primary && !event.shiftKey && event.key === "[") return "workbench.previous_tab";
  if (primary && event.shiftKey && event.key === "]") return "workbench.next_group";
  if (primary && event.shiftKey && event.key === "[") return "workbench.previous_group";
  if (primary && event.altKey && event.key === "ArrowRight") return "workbench.split_right";
  if (primary && event.altKey && event.key === "ArrowDown") return "workbench.split_down";
  if (event.altKey && event.shiftKey && event.key === "ArrowRight") {
    return "workbench.move_tab_next_group";
  }
  if (event.altKey && event.shiftKey && event.key === "ArrowLeft") {
    return "workbench.move_tab_previous_group";
  }
  if (event.altKey && event.shiftKey && key === "z") return "workbench.toggle_group_zoom";
  if (primary && event.altKey && key === "l") return "workbench.focus_left_dock";
  if (primary && event.altKey && key === "r") return "workbench.focus_right_dock";
  if (primary && key === "0") return "workbench.focus_workbench";
  return null;
}

/** Routes every workbench shortcut through canonical store/navigation commands. */
export function useWorkbenchCommands(
  options: UseWorkbenchCommandsOptions,
): WorkbenchCommandRouter {
  const optionsRef = useRef(options);
  optionsRef.current = options;

  const isEnabled = useCallback((commandId: WorkbenchCommandId): boolean => {
    const current = optionsRef.current;
    const state = current.store.getState();
    if (state.reset_pending) return false;
    const document = state.document;
    const activeGroup = document.groups[document.active_group_id];
    if (!activeGroup) return false;
    const groupCount = groupIdsInTreeOrder(document.root).length;
    switch (commandId) {
      case "workbench.next_tab":
      case "workbench.previous_tab":
        return activeGroup.surface_ids.length > 1;
      case "workbench.next_group":
      case "workbench.previous_group":
        return groupCount > 1;
      case "workbench.move_tab_next_group":
      case "workbench.move_tab_previous_group":
        return activeGroup.active_surface_id !== null && groupCount > 1;
      case "workbench.close_surface":
        return activeGroup.active_surface_id !== null;
      case "workbench.reopen_closed_surface":
        return document.recently_closed.length > 0;
      case "workbench.command_palette":
        return current.on_command_palette !== undefined;
      case "workbench.focus_left_dock":
        return current.on_focus_left_dock !== undefined;
      case "workbench.focus_right_dock":
        return current.on_focus_right_dock !== undefined;
      default:
        return true;
    }
  }, []);

  const execute = useCallback(async (commandId: WorkbenchCommandId): Promise<boolean> => {
    if (!isEnabled(commandId)) return false;
    const current = optionsRef.current;
    const state = current.store.getState();
    if (state.reset_pending) return false;
    const document = state.document;
    const activeGroup = document.groups[document.active_group_id];
    const groupIds = groupIdsInTreeOrder(document.root);
    const focusActive = (groupId: string, surfaceId: string | null): void => {
      if (surfaceId) focusSurface(current.root_ref.current, surfaceId);
      else focusGroup(current.root_ref.current, groupId);
      window.requestAnimationFrame(() => {
        const latest = current.store.getState().document;
        if (
          latest.active_group_id !== groupId
          || latest.groups[groupId]?.active_surface_id !== surfaceId
        ) return;
        if (surfaceId) focusSurface(current.root_ref.current, surfaceId);
        else focusGroup(current.root_ref.current, groupId);
      });
    };
    const applyActive = (groupId: string, surfaceId: string | null): boolean => {
      const result = current.store.getState().apply_commands([{
        type: "set_active_surface",
        group_id: groupId,
        surface_id: surfaceId,
      }]);
      if (!result.accepted) return false;
      focusActive(groupId, surfaceId);
      return true;
    };
    const focusRelativeGroup = (delta: number): boolean => {
      if (groupIds.length === 0) return false;
      const index = groupIds.indexOf(document.active_group_id);
      const groupId = groupIds[wrappedIndex(index < 0 ? 0 : index, delta, groupIds.length)];
      const group = document.groups[groupId];
      return applyActive(groupId, group.active_surface_id ?? group.surface_ids[0] ?? null);
    };
    const moveActiveSurface = (delta: number): boolean => {
      if (!activeGroup.active_surface_id || groupIds.length < 2) return false;
      const index = groupIds.indexOf(activeGroup.group_id);
      const targetGroupId = groupIds[wrappedIndex(index, delta, groupIds.length)];
      const targetGroup = document.groups[targetGroupId];
      const result = current.store.getState().apply_commands([{
        type: "move_surface",
        surface_id: activeGroup.active_surface_id,
        group_id: targetGroupId,
        index: targetGroup.surface_ids.length,
      }]);
      if (!result.accepted) return false;
      focusSurface(current.root_ref.current, activeGroup.active_surface_id);
      return true;
    };

    switch (commandId) {
      case "workbench.next_tab":
      case "workbench.previous_tab": {
        if (activeGroup.surface_ids.length === 0) return false;
        const index = activeGroup.active_surface_id
          ? activeGroup.surface_ids.indexOf(activeGroup.active_surface_id)
          : 0;
        const delta = commandId === "workbench.next_tab" ? 1 : -1;
        const surfaceId = activeGroup.surface_ids[wrappedIndex(index, delta, activeGroup.surface_ids.length)];
        return applyActive(activeGroup.group_id, surfaceId);
      }
      case "workbench.next_group":
        return focusRelativeGroup(1);
      case "workbench.previous_group":
        return focusRelativeGroup(-1);
      case "workbench.split_right":
      case "workbench.split_down": {
        const createId = current.create_id ?? defaultCreateId;
        const result = current.store.getState().apply_commands([{
          type: "split_group",
          group_id: activeGroup.group_id,
          new_group_id: createId("group"),
          node_id: createId("node"),
          direction: commandId === "workbench.split_right" ? "horizontal" : "vertical",
          placement: "after",
        }]);
        return result.accepted;
      }
      case "workbench.move_tab_next_group":
        return moveActiveSurface(1);
      case "workbench.move_tab_previous_group":
        return moveActiveSurface(-1);
      case "workbench.close_surface":
        return activeGroup.active_surface_id
          ? await current.navigation.close(activeGroup.active_surface_id) === "allow"
          : false;
      case "workbench.reopen_closed_surface": {
        const result = current.store.getState().apply_commands([{ type: "reopen_closed_surface" }]);
        if (!result.accepted) return false;
        const next = current.store.getState().document;
        focusActive(next.active_group_id, next.groups[next.active_group_id].active_surface_id);
        return true;
      }
      case "workbench.reset_workbench":
        return await current.navigation.reset_workbench() === "allow";
      case "workbench.toggle_group_zoom":
        current.store.getState().set_zoomed_group_id(
          state.zoomed_group_id === activeGroup.group_id ? null : activeGroup.group_id,
        );
        return true;
      case "workbench.open_surface":
        current.store.getState().set_launcher_open(true);
        return true;
      case "workbench.quick_open":
        if (current.on_quick_open) current.on_quick_open();
        else current.store.getState().set_launcher_open(true);
        return true;
      case "workbench.command_palette":
        current.on_command_palette?.();
        return current.on_command_palette !== undefined;
      case "workbench.focus_left_dock":
        current.on_focus_left_dock?.();
        return current.on_focus_left_dock !== undefined;
      case "workbench.focus_right_dock":
        current.on_focus_right_dock?.();
        return current.on_focus_right_dock !== undefined;
      case "workbench.focus_workbench":
        focusActive(activeGroup.group_id, activeGroup.active_surface_id);
        return true;
    }
  }, [isEnabled]);

  useEffect(() => {
    if (options.enabled === false) return;
    const handleKeyDown = (event: KeyboardEvent): void => {
      const commandId = shortcutForEvent(event);
      if (!commandId) return;
      const isGlobalPalette = commandId === "workbench.command_palette"
        || commandId === "workbench.quick_open";
      const root = optionsRef.current.root_ref.current;
      if (!isGlobalPalette
        && (!(event.target instanceof Node) || !root?.contains(event.target))) return;
      const isGroupTraversal = event.key === "F6";
      if (!isGroupTraversal && !isGlobalPalette
        && (isEditableTarget(event.target) || terminalOwnsShortcut(event.target))) {
        return;
      }
      event.preventDefault();
      if (isGroupTraversal) event.stopImmediatePropagation();
      void execute(commandId);
    };
    window.addEventListener("keydown", handleKeyDown, true);
    return () => window.removeEventListener("keydown", handleKeyDown, true);
  }, [execute, options.enabled]);

  return useMemo(
    () => ({ actions: WORKBENCH_COMMAND_ACTIONS, is_enabled: isEnabled, execute }),
    [execute, isEnabled],
  );
}
