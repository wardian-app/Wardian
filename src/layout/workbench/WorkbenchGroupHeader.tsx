import {
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type MouseEvent,
} from "react";
import { createPortal } from "react-dom";
import { Ellipsis, Plus } from "lucide-react";

export type WorkbenchPaneTarget = {
  group_id: string;
  position: "previous" | "next";
};

export type WorkbenchMenuItem = {
  label: string;
  on_select: () => void;
  danger?: boolean;
};

export type WorkbenchContextMenuProps = {
  aria_label: string;
  items: readonly WorkbenchMenuItem[];
  position: { x: number; y: number };
  on_close: () => void;
};

export function WorkbenchContextMenu({
  aria_label,
  items,
  position,
  on_close,
}: WorkbenchContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null);
  const width = 224;
  const estimatedHeight = Math.max(40, items.length * 32 + 8);
  const style: CSSProperties = {
    left: Math.max(4, Math.min(position.x, window.innerWidth - width - 4)),
    top: Math.max(4, Math.min(position.y, window.innerHeight - estimatedHeight - 4)),
  };

  useEffect(() => {
    const closeOnOutsidePointer = (event: PointerEvent): void => {
      if (!menuRef.current?.contains(event.target as Node)) on_close();
    };
    const closeOnEscape = (event: globalThis.KeyboardEvent): void => {
      if (event.key === "Escape") on_close();
    };
    const closeOnViewportChange = (): void => on_close();
    document.addEventListener("pointerdown", closeOnOutsidePointer);
    document.addEventListener("keydown", closeOnEscape);
    window.addEventListener("blur", closeOnViewportChange);
    window.addEventListener("resize", closeOnViewportChange);
    document.addEventListener("wheel", closeOnViewportChange, { passive: true });
    menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
    return () => {
      document.removeEventListener("pointerdown", closeOnOutsidePointer);
      document.removeEventListener("keydown", closeOnEscape);
      window.removeEventListener("blur", closeOnViewportChange);
      window.removeEventListener("resize", closeOnViewportChange);
      document.removeEventListener("wheel", closeOnViewportChange);
    };
  }, [on_close]);

  const moveFocus = (event: KeyboardEvent<HTMLDivElement>): void => {
    const menuItems = [...event.currentTarget.querySelectorAll<HTMLButtonElement>(
      '[role="menuitem"]',
    )];
    if (menuItems.length === 0) return;
    const current = menuItems.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.key === "ArrowDown"
      ? (current + 1) % menuItems.length
      : event.key === "ArrowUp"
        ? (current - 1 + menuItems.length) % menuItems.length
        : event.key === "Home"
          ? 0
          : event.key === "End"
            ? menuItems.length - 1
            : null;
    if (next === null) return;
    event.preventDefault();
    menuItems[next]?.focus();
  };

  return createPortal(
    <div
      ref={menuRef}
      role="menu"
      aria-label={aria_label}
      className="wardian-workbench-context-menu"
      style={style}
      onKeyDown={moveFocus}
      onContextMenu={(event) => event.preventDefault()}
    >
      {items.map((item) => (
        <button
          key={item.label}
          type="button"
          role="menuitem"
          className={item.danger ? "is-danger" : undefined}
          onClick={(event) => {
            event.stopPropagation();
            item.on_select();
            on_close();
          }}
        >
          {item.label}
        </button>
      ))}
    </div>,
    document.body,
  );
}

export type WorkbenchGroupHeaderProps = {
  group_id: string;
  pane_targets?: readonly WorkbenchPaneTarget[];
  is_zoomed?: boolean;
  on_open_surface?: (groupId: string) => void;
  on_toggle_zoom?: (groupId: string) => void;
  on_split_group?: (groupId: string, direction: "horizontal" | "vertical") => void;
  on_close_group?: (groupId: string) => void;
  on_join_group?: (sourceGroupId: string, targetGroupId: string) => void;
};

export function WorkbenchGroupHeader({
  group_id,
  pane_targets = [],
  is_zoomed = false,
  on_open_surface,
  on_toggle_zoom,
  on_split_group,
  on_close_group,
  on_join_group,
}: WorkbenchGroupHeaderProps) {
  const [menuPosition, setMenuPosition] = useState<{ x: number; y: number } | null>(null);
  const openMenu = (event: MouseEvent<HTMLButtonElement>): void => {
    event.stopPropagation();
    const bounds = event.currentTarget.getBoundingClientRect();
    setMenuPosition({ x: bounds.right - 224, y: bounds.bottom + 2 });
  };
  const menuItems: WorkbenchMenuItem[] = [
    {
      label: is_zoomed ? "Restore pane" : "Zoom pane",
      on_select: () => on_toggle_zoom?.(group_id),
    },
    { label: "Split pane right", on_select: () => on_split_group?.(group_id, "horizontal") },
    { label: "Split pane down", on_select: () => on_split_group?.(group_id, "vertical") },
    ...pane_targets.map((target): WorkbenchMenuItem => ({
      label: `Merge into ${target.position} pane`,
      on_select: () => on_join_group?.(group_id, target.group_id),
    })),
    { label: "Close pane", danger: true, on_select: () => on_close_group?.(group_id) },
  ];

  return (
    <div className="wardian-workbench-group-actions">
      <button
        type="button"
        className="wardian-workbench-header-action"
        aria-label="Open Surface"
        title="Open Surface"
        onClick={(event) => {
          event.stopPropagation();
          on_open_surface?.(group_id);
        }}
      >
        <Plus aria-hidden="true" size={16} strokeWidth={1.75} />
      </button>
      <button
        type="button"
        className="wardian-workbench-header-action"
        aria-label="Pane actions"
        aria-haspopup="menu"
        aria-expanded={menuPosition !== null}
        title="Pane actions"
        onClick={openMenu}
      >
        <Ellipsis aria-hidden="true" size={17} strokeWidth={1.75} />
      </button>
      {menuPosition && (
        <WorkbenchContextMenu
          aria_label="Pane actions"
          items={menuItems}
          position={menuPosition}
          on_close={() => setMenuPosition(null)}
        />
      )}
    </div>
  );
}
