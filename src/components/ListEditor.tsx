import React, { useState, useEffect } from 'react';
import { useConfirm } from './ConfirmDialog';

interface ValidatedInputProps {
  value: string;
  placeholder?: string;
  onChange?: (val: string) => void;
  validate?: (val: string) => Promise<boolean>;
  readOnly?: boolean;
}

export const ValidatedInput: React.FC<ValidatedInputProps> = ({ 
  value, 
  placeholder, 
  onChange, 
  validate,
  readOnly 
}) => {
  const [isValid, setIsValid] = useState<boolean | null>(null);

  useEffect(() => {
    if (validate) {
      validate(value).then(setIsValid);
    }
  }, [value, validate]);

  return (
    <div className="flex items-center gap-1 w-full bg-[var(--color-wardian-input-bg)] border border-wardian-light rounded px-2 py-1 transition-all focus-within:border-[var(--color-wardian-accent)]">
      <input
        className={`flex-1 bg-transparent text-xs ${readOnly ? 'text-muted-neutral/80 cursor-default' : 'text-primary'} focus:outline-none min-w-0`}
        value={value}
        placeholder={placeholder}
        onChange={(e) => !readOnly && onChange?.(e.target.value)}
        readOnly={readOnly}
      />
      {validate && value && (
        <span className="text-[10px] shrink-0" title={isValid ? "Valid path" : "Invalid or missing path"}>
          {isValid === true ? "✅" : isValid === false ? "⚠️" : ""}
        </span>
      )}
    </div>
  );
};

interface ListEditorProps {
  label: string;
  values: string[] | undefined;
  systemValues?: string[] | null;
  placeholder?: string;
  onChange: (values: string[]) => void;
  validate?: (val: string) => Promise<boolean>;
  onSystemValueDelete?: (idx: number) => void;
}

export const ListEditor: React.FC<ListEditorProps> = ({
  label,
  values = [],
  systemValues = [],
  placeholder,
  onChange,
  validate,
  onSystemValueDelete
}) => {
  const confirm = useConfirm();
  const safeValues = values || [];
  const safeSystemValues = systemValues || [];

  return (
    <div className="flex flex-col gap-1.5">
      <label className="block text-[10px] font-bold text-muted-neutral">{label}</label>
      <div className="flex flex-col gap-1">
        {/* System Values (Locked) */}
        {safeSystemValues.map((val, idx) => (
          <div key={`sys-${idx}`} className="flex items-center gap-1 group">
            <ValidatedInput
              value={val}
              readOnly
              validate={validate}
            />
            <button
              type="button"
              onClick={async () => {
                if (await confirm("This is a system-managed directory. Are you sure you want to remove it?")) {
                  onSystemValueDelete?.(idx);
                }
              }}
              className="text-muted-neutral hover:text-wardian-error p-0.5 transition-colors shrink-0"
              title="Remove system directory"
            >
              <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>
        ))}

        {/* User Values */}
        {safeValues.map((val, idx) => (
          <div key={`user-${idx}`} className="flex items-center gap-1 group">
            <ValidatedInput
              value={val}
              placeholder={placeholder}
              onChange={(newVal) => {
                const newValues = [...safeValues];
                newValues[idx] = newVal;
                onChange(newValues);
              }}
              validate={validate}
            />
            <button
              type="button"
              onClick={() => onChange(safeValues.filter((_, i) => i !== idx))}
              className="text-muted-neutral hover:text-red-400 p-0.5 transition-colors shrink-0"
              aria-label={`Remove ${label.toLowerCase()} value ${idx + 1}`}
              title={`Remove ${label.toLowerCase()} value ${idx + 1}`}
            >
              <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>
        ))}
        <button
          type="button"
          onClick={() => onChange([...safeValues, ""])}
          className="text-[10px] text-[var(--color-wardian-accent)] hover:text-primary transition-colors flex items-center gap-1 mt-0.5 w-fit"
        >
          <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 4v16m8-8H4" />
          </svg>
          Add {label.toLowerCase()}
        </button>
      </div>
    </div>
  );
};
