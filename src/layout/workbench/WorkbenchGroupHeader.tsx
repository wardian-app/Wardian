export type WorkbenchGroupHeaderProps = {
  group_id: string;
  join_target_ids?: readonly string[];
  on_open_surface?: (groupId: string) => void;
  on_toggle_zoom?: (groupId: string) => void;
  on_split_group?: (groupId: string, direction: "horizontal" | "vertical") => void;
  on_close_group?: (groupId: string) => void;
  on_join_group?: (sourceGroupId: string, targetGroupId: string) => void;
};

export function WorkbenchGroupHeader({
  group_id,
  join_target_ids = [],
  on_open_surface,
  on_toggle_zoom,
  on_split_group,
  on_close_group,
  on_join_group,
}: WorkbenchGroupHeaderProps) {
  return (
    <div className="wardian-workbench-group-actions">
      <button
        type="button"
        aria-label="Open Surface"
        onClick={() => on_open_surface?.(group_id)}
      >
        +
      </button>
      <button
        type="button"
        aria-label={`Toggle zoom for ${group_id}`}
        onClick={() => on_toggle_zoom?.(group_id)}
      >
        Zoom
      </button>
      <button
        type="button"
        aria-label={`Split ${group_id} right`}
        onClick={() => on_split_group?.(group_id, "horizontal")}
      >
        Split right
      </button>
      <button
        type="button"
        aria-label={`Split ${group_id} down`}
        onClick={() => on_split_group?.(group_id, "vertical")}
      >
        Split down
      </button>
      <button
        type="button"
        aria-label={`Close ${group_id}`}
        onClick={() => on_close_group?.(group_id)}
      >
        Close group
      </button>
      {join_target_ids.map((targetGroupId) => (
        <button
          key={targetGroupId}
          type="button"
          aria-label={`Join ${group_id} into ${targetGroupId}`}
          onClick={() => on_join_group?.(group_id, targetGroupId)}
        >
          Join {targetGroupId}
        </button>
      ))}
    </div>
  );
}
