import {
  useCallback,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type RefObject,
} from "react";

import { HomeSurface } from "../../features/workbench/HomeSurface";
import { WorkbenchCommandPalette } from "../../features/workbench/WorkbenchCommandPalette";
import {
  OpenSurfaceDialog,
  createCoreWorkbenchSurfaceRegistry,
} from "../../features/workbench/OpenSurfaceDialog";
import {
  createWorkbenchNavigationService,
  type WorkbenchIdKind,
  type WorkbenchNavigationService,
} from "../../features/workbench/navigationService";
import type { WorkbenchCommand } from "../../features/workbench/workbenchModel";
import type { WorkbenchSurfaceRegistry } from "../../features/workbench/surfaceRegistry";
import { useWorkbenchCommands } from "../../features/workbench/useWorkbenchCommands";
import type { WorkbenchStore } from "../../features/workbench/useWorkbenchStore";
import type { WorkbenchNewTabAction } from "../../types/settings";
import {
  canSplitWorkbenchPane,
  DockviewLayoutAdapter,
  type WorkbenchPanelRendererPolicy,
  type WorkbenchSurfaceIcon,
  type WorkbenchSurfaceRenderer,
  type WorkbenchSurfaceTitle,
} from "./DockviewLayoutAdapter";
import "./workbench.css";

export type WorkbenchHostProps = {
  store: WorkbenchStore;
  safe_mode?: boolean;
  registry?: WorkbenchSurfaceRegistry;
  navigation?: WorkbenchNavigationService;
  resource_key?: string;
  render_surface?: WorkbenchSurfaceRenderer;
  surface_title?: WorkbenchSurfaceTitle;
  on_quick_open?: () => void;
  on_focus_left_dock?: () => void;
  on_focus_right_dock?: () => void;
  create_id?: (kind: WorkbenchIdKind) => string;
  new_tab_action?: WorkbenchNewTabAction;
  root_ref?: RefObject<HTMLDivElement | null>;
};

type WorkbenchDropPosition = "top" | "bottom" | "left" | "right" | "center";

/** Applies the shared split predicate to a canonical pane's current DOM geometry. */
export function canSplitWorkbenchGroup(
  root: HTMLElement | null,
  groupId: string,
  direction: "horizontal" | "vertical",
): boolean {
  const group = [...(root?.querySelectorAll<HTMLElement>('[data-testid="workbench-group"]') ?? [])]
    .find((candidate) => candidate.dataset.groupId === groupId);
  return canSplitWorkbenchPane(
    group?.getBoundingClientRect(),
    direction === "horizontal" ? "right" : "bottom",
  );
}

function defaultCreateId(kind: WorkbenchIdKind): string {
  return `${kind}-${globalThis.crypto.randomUUID()}`;
}

/** Converts an edge drop into one canonical split-and-move transaction. */
export function workbenchEdgeDropCommands(
  surfaceId: string,
  targetGroupId: string,
  position: WorkbenchDropPosition,
  createId: (kind: WorkbenchIdKind) => string = defaultCreateId,
): readonly WorkbenchCommand[] {
  if (position === "center") return [];
  const newGroupId = createId("group");
  return [
    {
      type: "split_group",
      group_id: targetGroupId,
      new_group_id: newGroupId,
      node_id: createId("node"),
      direction: position === "left" || position === "right" ? "horizontal" : "vertical",
      placement: position === "left" || position === "top" ? "before" : "after",
    },
    {
      type: "move_surface",
      surface_id: surfaceId,
      group_id: newGroupId,
      index: 0,
    },
  ];
}

/** Subscribes the renderer and command boundaries to one canonical store. */
export function WorkbenchHost({
  store,
  safe_mode = false,
  registry: suppliedRegistry,
  navigation: suppliedNavigation,
  resource_key,
  render_surface,
  surface_title,
  on_quick_open,
  on_focus_left_dock,
  on_focus_right_dock,
  create_id,
  new_tab_action = "home",
  root_ref: suppliedRootRef,
}: WorkbenchHostProps) {
  const ownedRootRef = useRef<HTMLDivElement>(null);
  const rootRef = suppliedRootRef ?? ownedRootRef;
  const ownedRegistry = useMemo(createCoreWorkbenchSurfaceRegistry, []);
  const registry = suppliedRegistry ?? ownedRegistry;
  const subscribePresentation = useCallback(
    (listener: () => void) => registry.subscribe_presentation(listener),
    [registry],
  );
  const getPresentationVersion = useCallback(
    () => registry.presentation_version(),
    [registry],
  );
  const presentationVersion = useSyncExternalStore(
    subscribePresentation,
    getPresentationVersion,
    getPresentationVersion,
  );
  const ownedNavigation = useMemo(
    () => createWorkbenchNavigationService({
      registry,
      store,
      ...(create_id ? { create_id } : {}),
      can_split_group: (groupId, direction) => (
        canSplitWorkbenchGroup(rootRef.current, groupId, direction)
      ),
    }),
    [create_id, registry, rootRef, store],
  );
  const navigation = suppliedNavigation ?? ownedNavigation;
  const [launcherGroupId, setLauncherGroupId] = useState<string | null>(null);
  const [launcherPlaceholderId, setLauncherPlaceholderId] = useState<string | null>(null);
  const launcherReturnFocusRef = useRef<HTMLElement | null>(null);
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);
  const state = useSyncExternalStore(
    (listener) => store.subscribe(() => listener()),
    store.getState,
    store.getInitialState,
  );
  const openCommandPalette = useCallback(() => setCommandPaletteOpen(true), []);
  const canSplitGroup = useCallback((
    groupId: string,
    direction: "horizontal" | "vertical",
  ): boolean => canSplitWorkbenchGroup(rootRef.current, groupId, direction), []);

  const openNewTabLauncher = useCallback((groupId: string) => {
    if (new_tab_action === "home") {
      navigation.open({ surface_type: "new-tab", group_id: groupId });
      return;
    }
    launcherReturnFocusRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    setLauncherGroupId(groupId);
    setLauncherPlaceholderId(null);
    store.getState().set_launcher_open(true);
  }, [navigation, new_tab_action, store]);
  const openPaletteForGroup = useCallback((groupId: string) => {
    launcherReturnFocusRef.current = null;
    setLauncherGroupId(groupId);
    setLauncherPlaceholderId(null);
    store.getState().set_launcher_open(true);
  }, [store]);
  const openPaletteForPlaceholder = useCallback((groupId: string, surfaceId: string) => {
    launcherReturnFocusRef.current = null;
    setLauncherGroupId(groupId);
    setLauncherPlaceholderId(surfaceId);
    store.getState().set_launcher_open(true);
  }, [store]);
  const requestSearchableLauncher = useCallback(() => {
    if (!store.getState().launcher_open) {
      launcherReturnFocusRef.current = null;
      setLauncherGroupId(store.getState().document.active_group_id);
    }
    setLauncherPlaceholderId(null);
    store.getState().set_launcher_open(true);
  }, [store]);
  const requestQuickOpen = useCallback(() => {
    requestSearchableLauncher();
    on_quick_open?.();
  }, [on_quick_open, requestSearchableLauncher]);
  const closeLauncher = useCallback(() => {
    store.getState().set_launcher_open(false);
    setLauncherGroupId(null);
    setLauncherPlaceholderId(null);
    launcherReturnFocusRef.current = null;
  }, [store]);
  const commands = useWorkbenchCommands({
    store,
    navigation,
    root_ref: rootRef,
    on_quick_open: requestQuickOpen,
    on_open_surface: requestSearchableLauncher,
    on_command_palette: openCommandPalette,
    on_focus_left_dock,
    on_focus_right_dock,
    create_id,
    can_split_group: canSplitGroup,
  });
  const activateGroup = useCallback((groupId: string): boolean => {
    const document = store.getState().document;
    const group = document.groups[groupId];
    if (!group) return false;
    if (document.active_group_id === groupId) return true;
    return store.getState().apply_commands([{
      type: "set_active_surface",
      group_id: groupId,
      surface_id: group.active_surface_id,
    }]).accepted;
  }, [store]);

  const defaultRenderSurface = useCallback<WorkbenchSurfaceRenderer>((surface) => {
    const presentation = registry.presentation(surface);
    return (
      <section className="wardian-workbench-placeholder">
        <h2>{presentation.title}</h2>
        <p>This registered surface will adopt its existing Wardian view in the migration phase.</p>
      </section>
    );
  }, [registry]);
  const defaultTitleSurface = useCallback<WorkbenchSurfaceTitle>(
    (surface) => registry.presentation(surface).title,
    [presentationVersion, registry],
  );
  const surfaceIcon = useCallback<WorkbenchSurfaceIcon>(
    (surface) => registry.presentation(surface).icon,
    [presentationVersion, registry],
  );
  const rendererPolicy = useCallback<WorkbenchPanelRendererPolicy>((surface) => (
      registry.get(surface.surface_type)?.render_policy === "recreate_from_state"
        ? "onlyWhenVisible" as const
        : "always" as const
    ), [registry]);
  const baseRenderSurface = render_surface ?? defaultRenderSurface;
  const renderSurface = useCallback<WorkbenchSurfaceRenderer>((surface, context) => {
    if (surface.surface_type !== "new-tab") return baseRenderSurface(surface, context);
    const groupId = Object.values(state.document.groups).find(
      (group) => group.surface_ids.includes(surface.surface_id),
    )?.group_id ?? state.document.active_group_id;
    return (
      <HomeSurface
        group_id={groupId}
        registry={registry}
        recently_closed={state.document.recently_closed}
        on_open_surface={(targetGroupId) => {
          openPaletteForPlaceholder(targetGroupId, surface.surface_id);
        }}
        on_select_surface={(surfaceType) => {
          navigation.open_from_placeholder(surface.surface_id, { surface_type: surfaceType });
        }}
        on_reopen_closed={() => {
          navigation.reopen_closed_from_placeholder(surface.surface_id);
        }}
      />
    );
  }, [
    baseRenderSurface,
    navigation,
    openPaletteForPlaceholder,
    registry,
    state.document,
    store,
  ]);
  const titleSurface = useCallback<WorkbenchSurfaceTitle>(
    (surface) => (surface_title ?? defaultTitleSurface)(surface),
    [defaultTitleSurface, presentationVersion, surface_title],
  );

  return (
    <div
      ref={rootRef}
      aria-busy={state.reset_pending}
      inert={state.reset_pending ? true : undefined}
      data-reset-pending={state.reset_pending ? "true" : "false"}
      data-testid="workbench-host"
      data-zoomed-group-id={state.zoomed_group_id ?? "none"}
      className="wardian-workbench-host"
    >
      <DockviewLayoutAdapter
        document={state.document}
        safe_mode={safe_mode}
        zoomed_group_id={state.zoomed_group_id}
        render_surface={renderSurface}
        surface_title={titleSurface}
        surface_icon={surfaceIcon}
        renderer_policy={rendererPolicy}
        render_home={(groupId) => (
          <HomeSurface
            group_id={groupId}
            registry={registry}
            recently_closed={state.document.recently_closed}
            on_open_surface={openPaletteForGroup}
            on_select_surface={(surfaceType, targetGroupId) => {
              navigation.open({ surface_type: surfaceType, group_id: targetGroupId });
            }}
            on_reopen_closed={() => { void commands.execute("workbench.reopen_closed_surface"); }}
          />
        )}
        on_command={(command) => store.getState().apply_commands([command]).accepted}
        on_open_surface={openNewTabLauncher}
        on_toggle_zoom={(groupId) => {
          if (!activateGroup(groupId)) return;
          void commands.execute("workbench.toggle_group_zoom");
        }}
        on_split_group={(groupId, direction) => {
          if (!canSplitGroup(groupId, direction)) return;
          if (!activateGroup(groupId)) return;
          void commands.execute(
            direction === "horizontal" ? "workbench.split_right" : "workbench.split_down",
          );
        }}
        on_close_group={(groupId) => { void navigation.close_group(groupId); }}
        on_close_surface={(surfaceId) => { void navigation.close(surfaceId); }}
        on_join_group={(sourceGroupId, targetGroupId) => {
          store.getState().apply_commands([{
            type: "join_group",
            source_group_id: sourceGroupId,
            target_group_id: targetGroupId,
          }]);
        }}
        on_surface_drop={(surfaceId, targetGroupId, position) => {
          if (position !== "center" && !canSplitGroup(
            targetGroupId,
            position === "left" || position === "right" ? "horizontal" : "vertical",
          )) return;
          store.getState().apply_commands(workbenchEdgeDropCommands(
            surfaceId,
            targetGroupId,
            position,
            create_id ?? defaultCreateId,
          ));
        }}
      />
      <OpenSurfaceDialog
        open={state.launcher_open}
        group_id={launcherGroupId ?? state.document.active_group_id}
        resource_key={resource_key}
        navigation={navigation}
        registry={registry}
        recently_closed={state.document.recently_closed}
        on_reopen_closed={() => { void commands.execute("workbench.reopen_closed_surface"); }}
        on_close={closeLauncher}
        return_focus={launcherReturnFocusRef.current}
        placeholder_surface_id={launcherPlaceholderId ?? undefined}
      />
      <WorkbenchCommandPalette
        open={commandPaletteOpen}
        actions={commands.actions}
        is_enabled={commands.is_enabled}
        on_execute={commands.execute}
        on_close={() => setCommandPaletteOpen(false)}
      />
    </div>
  );
}
