import { useEffect, useRef } from "react";
import type { OptionalColumnId, WatchlistPrefs } from "./types";

const COLUMN_LABELS: Record<OptionalColumnId, string> = {
  status_label: "Status",
  query_count: "Query Count",
  uptime: "Uptime",
  provider_model: "Provider / Model",
  last_queried: "Last Queried",
};

interface ColumnPickerProps {
  prefs: WatchlistPrefs;
  onPrefsChange: (prefs: WatchlistPrefs) => void;
  onClose: () => void;
}

export function ColumnPicker({ prefs, onPrefsChange, onClose }: ColumnPickerProps) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        onClose();
      }
    }
    document.addEventListener("click", handleClick);
    return () => document.removeEventListener("click", handleClick);
  }, [onClose]);

  function toggleColumn(id: OptionalColumnId) {
    const updated = prefs.columns.map(c =>
      c.id === id ? { ...c, visible: !c.visible } : c
    );
    onPrefsChange({ ...prefs, columns: updated });
  }

  return (
    <div
      ref={ref}
      className="absolute right-0 top-full mt-1 z-50 border border-[var(--color-wardian-border-heavy)] rounded-lg shadow-[0_8px_32px_rgba(0,0,0,0.2)] p-1 min-w-[160px]"
      style={{ background: 'var(--color-wardian-sidebar-secondary)' }}
    >
      <p className="label-small text-muted mb-2">Columns</p>
      {prefs.columns.map(col => (
        <label
          key={col.id}
          className="flex items-center gap-2 py-1 px-1 cursor-pointer hover:bg-wardian-hover rounded"
        >
          <input
            type="checkbox"
            checked={col.visible}
            onChange={() => toggleColumn(col.id)}
            className="accent-wardian-accent"
          />
          <span className="label-small">{COLUMN_LABELS[col.id]}</span>
        </label>
      ))}
    </div>
  );
}
