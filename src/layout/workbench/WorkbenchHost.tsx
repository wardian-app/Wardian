import {
  useCallback,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";

import { HomeSurface } from "../../features/workbench/HomeSurface";
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
import {
  useWorkbenchCommands,
  type WorkbenchCommandId,
} from "../../features/workbench/useWorkbenchCommands";
import type { WorkbenchStore } from "../../features/workbench/useWorkbenchStore";
import {
  DockviewLayoutAdapter,
  type WorkbenchSurfaceRenderer,
} from "./DockviewLayoutAdapter";
import "./workbench.css";

export type WorkbenchHostProps = {
  store: WorkbenchStore;
  safe_mode?: boolean;
  registry?: WorkbenchSurfaceRegistry;
  navigation?: WorkbenchNavigationService;
  resource_key?: string;
  render_surface?: WorkbenchSurfaceRenderer;
  on_quick_open?: () => void;
  on_command_palette?: () => void;
  on_focus_left_dock?: () => void;
  on_focus_right_dock?: () => void;
  create_id?: (kind: WorkbenchIdKind) => string;
};

type WorkbenchDropPosition = "top" | "bottom" | "left" | "right" | "center";

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

const VISIBLE_COMMANDS: readonly WorkbenchCommandId[] = [
  "workbench.quick_open",
  "workbench.split_right",
  "workbench.split_down",
  "workbench.move_tab_previous_group",
  "workbench.move_tab_next_group",
  "workbench.close_surface",
  "workbench.reopen_closed_surface",
  "workbench.toggle_group_zoom",
  "workbench.focus_left_dock",
  "workbench.focus_right_dock",
];

/** Subscribes the renderer and command boundaries to one canonical store. */
export function WorkbenchHost({
  store,
  safe_mode = false,
  registry: suppliedRegistry,
  navigation: suppliedNavigation,
  resource_key,
  render_surface,
  on_quick_open,
  on_command_palette,
  on_focus_left_dock,
  on_focus_right_dock,
  create_id,
}: WorkbenchHostProps) {
  const rootRef = useRef<HTMLDivElement>(null);
  const ownedRegistry = useMemo(createCoreWorkbenchSurfaceRegistry, []);
  const registry = suppliedRegistry ?? ownedRegistry;
  const ownedNavigation = useMemo(
    () => createWorkbenchNavigationService({
      registry,
      store,
      ...(create_id ? { create_id } : {}),
    }),
    [create_id, registry, store],
  );
  const navigation = suppliedNavigation ?? ownedNavigation;
  const [launcherGroupId, setLauncherGroupId] = useState<string | null>(null);
  const state = useSyncExternalStore(
    (listener) => store.subscribe(() => listener()),
    store.getState,
    store.getInitialState,
  );
  const commands = useWorkbenchCommands({
    store,
    navigation,
    root_ref: rootRef,
    on_quick_open,
    on_command_palette: on_command_palette ?? (() => store.getState().set_launcher_open(true)),
    on_focus_left_dock,
    on_focus_right_dock,
    create_id,
  });

  const openLauncher = useCallback((groupId: string) => {
    setLauncherGroupId(groupId);
    store.getState().set_launcher_open(true);
  }, [store]);
  const closeLauncher = useCallback(() => {
    store.getState().set_launcher_open(false);
    setLauncherGroupId(null);
  }, [store]);
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

  const renderSurface = render_surface ?? ((surface) => {
    const presentation = registry.presentation(surface);
    return (
      <section className="wardian-workbench-placeholder">
        <h2>{presentation.title}</h2>
        <p>This registered surface will adopt its existing Wardian view in the migration phase.</p>
      </section>
    );
  });

  return (
    <div
      ref={rootRef}
      data-testid="workbench-host"
      data-zoomed-group-id={state.zoomed_group_id ?? "none"}
      className="wardian-workbench-host"
    >
      <nav aria-label="Workbench commands" className="wardian-workbench-command-bar">
        {commands.actions.filter((action) => VISIBLE_COMMANDS.includes(action.command_id)).map((action) => (
          <button
            key={action.command_id}
            type="button"
            onClick={() => { void commands.execute(action.command_id); }}
          >
            {action.title}
          </button>
        ))}
      </nav>
      <DockviewLayoutAdapter
        document={state.document}
        safe_mode={safe_mode}
        zoomed_group_id={state.zoomed_group_id}
        render_surface={renderSurface}
        renderer_policy={(surface) => registry.get(surface.surface_type)?.render_policy === "keep_alive"
          ? "always"
          : "onlyWhenVisible"}
        render_home={(groupId) => (
          <HomeSurface
            group_id={groupId}
            recently_closed={state.document.recently_closed}
            on_open_surface={openLauncher}
            on_reopen_closed={() => { void commands.execute("workbench.reopen_closed_surface"); }}
          />
        )}
        on_command={(command) => store.getState().apply_commands([command]).accepted}
        on_open_surface={openLauncher}
        on_toggle_zoom={(groupId) => {
          if (!activateGroup(groupId)) return;
          void commands.execute("workbench.toggle_group_zoom");
        }}
        on_split_group={(groupId, direction) => {
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
      />
    </div>
  );
}
