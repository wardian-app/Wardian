import React from 'react';
import type { ScheduleDefinition } from '../../types/workflow';

const SCHEDULE_TYPES = [
  { value: 'interval', label: 'Interval' },
  { value: 'daily', label: 'Daily' },
  { value: 'weekly', label: 'Weekly' },
  { value: 'monthly', label: 'Monthly' },
  { value: 'specific_dates', label: 'Specific Dates' },
  { value: 'one_time', label: 'One-Time' },
] as const;

const DAY_LABELS = [
  { key: 'Sun', label: 'S' },
  { key: 'Mon', label: 'M' },
  { key: 'Tue', label: 'T' },
  { key: 'Wed', label: 'W' },
  { key: 'Thu', label: 'T' },
  { key: 'Fri', label: 'F' },
  { key: 'Sat', label: 'S' },
];

const END_CONDITIONS = [
  { value: 'never', label: 'Never' },
  { value: 'on_date', label: 'On date' },
  { value: 'after_occurrences', label: 'After' },
] as const;

interface ScheduleEditorProps {
  value: Partial<ScheduleDefinition>;
  onChange: (value: Partial<ScheduleDefinition>) => void;
  compact?: boolean;
}

const inputClass = "w-full bg-[var(--color-wardian-input-bg)] border border-wardian-border rounded-lg px-3 py-1.5 text-[11px] text-[var(--color-wardian-text)] outline-none focus:border-[var(--color-wardian-accent)]/50 transition-colors";
const selectClass = inputClass + " cursor-pointer";
const labelClass = "text-[10px] font-bold text-muted-neutral uppercase tracking-wider";

export const ScheduleEditor: React.FC<ScheduleEditorProps> = ({ value, onChange, compact }) => {
  const schedType = value.schedule_type || 'interval';

  const update = (patch: Partial<ScheduleDefinition>) => {
    onChange({ ...value, ...patch });
  };

  // For interval: allow toggling between minutes and hours in the UI
  const intervalMinutes = value.interval_minutes || 60;
  const isHoursMode = intervalMinutes >= 60 && intervalMinutes % 60 === 0;
  const displayInterval = isHoursMode ? intervalMinutes / 60 : intervalMinutes;
  const intervalUnit = isHoursMode ? 'hours' : 'minutes';

  const daysOfWeek = value.days_of_week || [];
  const daysOfMonth = value.days_of_month || [];
  const specificDates = value.specific_dates || [];
  const endCondition = value.end_condition || 'never';

  const showEndCondition = schedType !== 'one_time' && schedType !== 'specific_dates';

  const radioGroupName = React.useId();

  return (
    <div className={`space-y-3 ${compact ? '' : 'p-3'}`}>
      {/* Schedule Type Selector */}
      <div className="space-y-1">
        <label className={labelClass}>Schedule Type</label>
        <select
          value={schedType}
          onChange={(e) => update({ schedule_type: e.target.value as ScheduleDefinition['schedule_type'] })}
          className={selectClass}
        >
          {SCHEDULE_TYPES.map(t => (
            <option key={t.value} value={t.value}>{t.label}</option>
          ))}
        </select>
      </div>

      {/* === INTERVAL === */}
      {schedType === 'interval' && (
        <div className="space-y-1">
          <label className={labelClass}>Repeat every</label>
          <div className="flex gap-2 items-center">
            <input
              type="number"
              min={1}
              value={displayInterval}
              onChange={(e) => {
                const num = parseInt(e.target.value) || 1;
                update({ interval_minutes: intervalUnit === 'hours' ? num * 60 : num });
              }}
              className={inputClass + " w-20"}
            />
            <select
              value={intervalUnit}
              onChange={(e) => {
                const newUnit = e.target.value;
                if (newUnit === 'hours') {
                  update({ interval_minutes: Math.max(1, Math.round(intervalMinutes / 60)) * 60 });
                } else {
                  update({ interval_minutes: isHoursMode ? intervalMinutes : intervalMinutes });
                }
              }}
              className={selectClass + " w-24"}
            >
              <option value="minutes">minutes</option>
              <option value="hours">hours</option>
            </select>
          </div>
        </div>
      )}

      {/* === DAILY === */}
      {schedType === 'daily' && (
        <div className="space-y-1">
          <label className={labelClass}>At time</label>
          <input
            type="time"
            value={value.time_of_day || '09:00'}
            onChange={(e) => update({ time_of_day: e.target.value })}
            className={inputClass}
          />
        </div>
      )}

      {/* === WEEKLY === */}
      {schedType === 'weekly' && (
        <div className="space-y-3">
          <div className="space-y-1">
            <label className={labelClass}>Repeat every</label>
            <div className="flex gap-2 items-center">
              <input
                type="number"
                min={1}
                value={value.repeat_every || 1}
                onChange={(e) => update({ repeat_every: parseInt(e.target.value) || 1 })}
                className={inputClass + " w-16"}
              />
              <span className="text-[11px] text-muted-neutral">week(s)</span>
            </div>
          </div>

          <div className="space-y-1.5">
            <label className={labelClass}>Repeat on</label>
            <div className="flex gap-1">
              {DAY_LABELS.map(({ key, label }) => {
                const active = daysOfWeek.includes(key);
                return (
                  <button
                    key={key}
                    type="button"
                    onClick={() => {
                      const next = active
                        ? daysOfWeek.filter(d => d !== key)
                        : [...daysOfWeek, key];
                      update({ days_of_week: next });
                    }}
                    className={`w-7 h-7 rounded-full text-[10px] font-bold transition-all cursor-pointer border ${
                      active
                        ? 'bg-[var(--color-wardian-accent)] text-[var(--color-wardian-bg)] border-[var(--color-wardian-accent)]'
                        : 'bg-transparent text-muted-neutral border-wardian-border hover:border-[var(--color-wardian-accent)]/50'
                    }`}
                    aria-label={key}
                    aria-pressed={active}
                  >
                    {label}
                  </button>
                );
              })}
            </div>
          </div>

          <div className="space-y-1">
            <label className={labelClass}>At time</label>
            <input
              type="time"
              value={value.time_of_day || '09:00'}
              onChange={(e) => update({ time_of_day: e.target.value })}
              className={inputClass}
            />
          </div>
        </div>
      )}

      {/* === MONTHLY === */}
      {schedType === 'monthly' && (
        <div className="space-y-3">
          <div className="space-y-1.5">
            <label className={labelClass}>On day(s) of month</label>
            <div className="flex flex-wrap gap-1">
              {Array.from({ length: 31 }, (_, i) => i + 1).map(day => {
                const active = daysOfMonth.includes(day);
                return (
                  <button
                    key={day}
                    type="button"
                    onClick={() => {
                      const next = active
                        ? daysOfMonth.filter(d => d !== day)
                        : [...daysOfMonth, day].sort((a, b) => a - b);
                      update({ days_of_month: next });
                    }}
                    className={`w-7 h-7 rounded-md text-[9px] font-bold transition-all cursor-pointer border ${
                      active
                        ? 'bg-[var(--color-wardian-accent)] text-[var(--color-wardian-bg)] border-[var(--color-wardian-accent)]'
                        : 'bg-transparent text-muted-neutral border-wardian-border hover:border-[var(--color-wardian-accent)]/50'
                    }`}
                  >
                    {day}
                  </button>
                );
              })}
            </div>
          </div>

          <div className="space-y-1">
            <label className={labelClass}>At time</label>
            <input
              type="time"
              value={value.time_of_day || '09:00'}
              onChange={(e) => update({ time_of_day: e.target.value })}
              className={inputClass}
            />
          </div>
        </div>
      )}

      {/* === SPECIFIC DATES === */}
      {schedType === 'specific_dates' && (
        <div className="space-y-3">
          <div className="space-y-1.5">
            <label className={labelClass}>Dates</label>
            {specificDates.map((date, i) => (
              <div key={i} className="flex gap-2 items-center">
                <input
                  type="date"
                  value={date}
                  onChange={(e) => {
                    const next = [...specificDates];
                    next[i] = e.target.value;
                    update({ specific_dates: next });
                  }}
                  className={inputClass + " flex-1"}
                />
                <button
                  type="button"
                  onClick={() => update({ specific_dates: specificDates.filter((_, j) => j !== i) })}
                  className="text-[var(--color-wardian-error)] hover:bg-[var(--color-wardian-error)]/10 p-1 rounded transition-all cursor-pointer"
                >
                  <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M6 18L18 6M6 6l12 12"/></svg>
                </button>
              </div>
            ))}
            <button
              type="button"
              onClick={() => {
                const today = new Date().toISOString().split('T')[0];
                update({ specific_dates: [...specificDates, today] });
              }}
              className="text-[10px] font-bold text-[var(--color-wardian-accent)] hover:underline cursor-pointer flex items-center gap-1"
            >
              <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 4v16m8-8H4"/></svg>
              Add Date
            </button>
          </div>

          <div className="space-y-1">
            <label className={labelClass}>At time</label>
            <input
              type="time"
              value={value.time_of_day || '09:00'}
              onChange={(e) => update({ time_of_day: e.target.value })}
              className={inputClass}
            />
          </div>
        </div>
      )}

      {/* === ONE-TIME === */}
      {schedType === 'one_time' && (
        <div className="space-y-1">
          <label className={labelClass}>Date & Time</label>
          <input
            type="datetime-local"
            aria-label="Date & Time"
            value={value.run_at || ''}
            onChange={(e) => update({ run_at: e.target.value })}
            className={inputClass}
          />
        </div>
      )}

      {/* === END CONDITION === */}
      {showEndCondition && (
        <div className="space-y-2 pt-2 border-t border-wardian-border/30">
          <label className={labelClass}>Ends</label>
          <div className="space-y-2">
            {END_CONDITIONS.map(({ value: ec, label }) => (
              <div key={ec} className="flex items-center gap-2">
                <input
                  type="radio"
                  name={`end_condition_${radioGroupName}`}
                  checked={endCondition === ec}
                  onChange={() => {
                    const payload: Partial<ScheduleDefinition> = { end_condition: ec as ScheduleDefinition['end_condition'] };
                    if (ec === 'after_occurrences' && !value.max_occurrences) payload.max_occurrences = 10;
                    if (ec === 'on_date' && !value.end_date) payload.end_date = new Date().toISOString().split('T')[0];
                    update(payload);
                  }}
                  aria-label={label}
                  className="accent-[var(--color-wardian-accent)] cursor-pointer"
                />
                <span className="text-[11px] text-[var(--color-wardian-text)]">{label}</span>

                {ec === 'on_date' && endCondition === 'on_date' && (
                  <input
                    type="date"
                    value={value.end_date || ''}
                    onChange={(e) => update({ end_date: e.target.value })}
                    className={inputClass + " w-36 ml-1"}
                  />
                )}

                {ec === 'after_occurrences' && endCondition === 'after_occurrences' && (
                  <div className="flex items-center gap-1 ml-1">
                    <input
                      type="number"
                      min={1}
                      value={value.max_occurrences || 10}
                      onChange={(e) => update({ max_occurrences: parseInt(e.target.value) || 10 })}
                      className={inputClass + " w-16"}
                    />
                    <span className="text-[10px] text-muted-neutral">occurrences</span>
                  </div>
                )}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
};
