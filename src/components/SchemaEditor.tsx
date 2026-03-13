import React, { useState, useEffect, useCallback } from 'react';
import { RenderableInput } from './RenderableInput';

interface SchemaEditorProps {
  value: string;
  onChange: (val: string) => void;
  nodeId: string;
}

const PROPERTY_TYPES = ['string', 'number', 'boolean', 'object', 'integer', 'array'];

interface PropertyRowProps {
  propKey: string;
  propValue: any;
  onRename: (oldKey: string, newKey: string) => void;
  onUpdate: (key: string, updates: any) => void;
  onRemove: (key: string) => void;
  onAddSub: () => void;
  renderNested: () => React.ReactNode;
}

const PropertyRow: React.FC<PropertyRowProps> = ({ 
  propKey, 
  propValue, 
  onRename, 
  onUpdate, 
  onRemove, 
  onAddSub,
  renderNested 
}) => {
  const [localName, setLocalName] = useState(propKey);

  // Sync local name if propKey changes from outside (e.g. undo or external edit)
  useEffect(() => {
    setLocalName(propKey);
  }, [propKey]);

  const handleBlur = () => {
    if (localName !== propKey) {
      onRename(propKey, localName);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      (e.target as HTMLInputElement).blur();
    }
  };

  return (
    <div className="flex flex-col gap-2 p-2 rounded-lg bg-[color-mix(in_srgb,var(--color-wardian-bg),black_10%)] border border-wardian-border transition-colors hover:border-[var(--color-wardian-accent)]/30">
      <div className="flex items-center gap-2">
        <input 
          className="flex-1 bg-transparent border-none outline-none text-[11px] font-bold text-[var(--color-wardian-text)] focus:text-[var(--color-wardian-accent)]"
          value={localName}
          placeholder="key"
          onChange={(e) => setLocalName(e.target.value)}
          onBlur={handleBlur}
          onKeyDown={handleKeyDown}
        />
        <select 
          className="bg-[var(--color-wardian-bg)] border border-wardian-border rounded px-1.5 py-0.5 text-[10px] font-mono text-[var(--color-wardian-processing)] font-bold outline-none cursor-pointer"
          value={propValue.type || 'string'}
          onChange={(e) => onUpdate(propKey, { type: e.target.value })}
        >
          {PROPERTY_TYPES.map(t => <option key={t} value={t} className="bg-[var(--color-wardian-card)] text-[var(--color-wardian-text)]">{t}</option>)}
        </select>
        <button 
          onClick={() => onRemove(propKey)}
          className="p-1 text-muted-neutral hover:text-red-500 transition-colors cursor-pointer"
        >
          <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"></path></svg>
        </button>
      </div>
      {propValue.type === 'object' && (
        <div className="mt-1">
          {renderNested()}
          <button 
            onClick={onAddSub}
            className="mt-2 text-[9px] font-bold text-[var(--color-wardian-accent)] uppercase tracking-widest hover:opacity-80 transition-all flex items-center gap-1.5 px-2 py-1 rounded border border-transparent hover:border-[var(--color-wardian-accent)]/20"
          >
            <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="3" d="M12 4v16m8-8H4"></path></svg>
            Add Subfield
          </button>
        </div>
      )}
    </div>
  );
};

export const SchemaEditor: React.FC<SchemaEditorProps> = ({ value, onChange, nodeId }) => {
  const [isCodeView, setIsCodeView] = useState(false);
  const [schemaObj, setSchemaObj] = useState<any>({ type: 'object', properties: {} });

  useEffect(() => {
    try {
      const parsed = JSON.parse(value || '{}');
      if (parsed.type === 'object' || parsed.properties) {
        setSchemaObj(parsed);
      } else if (Object.keys(parsed).length > 0 && !parsed.type) {
        setSchemaObj({ type: 'object', properties: parsed });
      }
    } catch (e) {}
  }, [value]);

  const updateSchema = useCallback((newSchema: any) => {
    setSchemaObj(newSchema);
    onChange(JSON.stringify(newSchema, null, 2));
  }, [onChange]);

  const handlePropertyUpdate = (key: string, updates: any, parent: any) => {
    const props = { ...(parent.properties || {}) };
    props[key] = { ...props[key], ...updates };
    if (updates.type === 'object' && !props[key].properties) {
      props[key].properties = {};
    }
    if (parent === schemaObj) {
      updateSchema({ ...schemaObj, properties: props });
    } else {
      parent.properties = props;
      updateSchema({ ...schemaObj });
    }
  };

  const handlePropertyRename = (oldKey: string, newKey: string, parent: any) => {
    if (oldKey === newKey || !newKey) return;
    const props = { ...(parent.properties || {}) };
    
    // Check for collisions
    if (props[newKey]) {
        // Simple collision avoidance: key_1, key_2...
        let finalKey = newKey;
        let counter = 1;
        while (props[finalKey]) {
            finalKey = `${newKey}_${counter++}`;
        }
        newKey = finalKey;
    }

    props[newKey] = props[oldKey];
    delete props[oldKey];
    if (parent === schemaObj) {
      updateSchema({ ...schemaObj, properties: props });
    } else {
      parent.properties = props;
      updateSchema({ ...schemaObj });
    }
  };

  const handlePropertyRemove = (key: string, parent: any) => {
    const props = { ...(parent.properties || {}) };
    delete props[key];
    if (parent === schemaObj) {
      updateSchema({ ...schemaObj, properties: props });
    } else {
      parent.properties = props;
      updateSchema({ ...schemaObj });
    }
  };

  const addProperty = (parent: any) => {
    const props = { ...(parent.properties || {}) };
    const key = `field_${Object.keys(props).length + 1}`;
    props[key] = { type: 'string' };
    if (parent === schemaObj) {
      updateSchema({ ...schemaObj, type: 'object', properties: props });
    } else {
      parent.properties = props;
      updateSchema({ ...schemaObj });
    }
  };

  const renderProperties = (obj: any, level = 0) => {
    const props = obj.properties || {};
    return (
      <div className={`space-y-2 ${level > 0 ? 'ml-6 pl-4 border-l border-wardian-border' : ''}`}>
        {Object.keys(props).map(key => (
          <PropertyRow 
            key={key}
            propKey={key}
            propValue={props[key]}
            onRename={(old, nw) => handlePropertyRename(old, nw, obj)}
            onUpdate={(k, u) => handlePropertyUpdate(k, u, obj)}
            onRemove={(k) => handlePropertyRemove(k, obj)}
            onAddSub={() => addProperty(props[key])}
            renderNested={() => renderProperties(props[key], level + 1)}
          />
        ))}
        {level === 0 && Object.keys(props).length === 0 && (
            <div className="flex flex-col items-center justify-center py-8 text-center bg-[color-mix(in_srgb,var(--color-wardian-bg),black_5%)] border border-dashed border-wardian-border rounded-xl">
              <span className="text-[10px] text-muted-neutral italic opacity-50 uppercase tracking-widest">No Fields Defined</span>
            </div>
        )}
      </div>
    );
  };

  return (
    <div className="flex flex-col gap-3">
        <div className="flex items-center justify-between bg-[var(--color-wardian-bg)] p-1 rounded-xl border border-wardian-border shadow-sm">
            <div className="flex gap-0.5">
                <button 
                    onClick={() => setIsCodeView(false)}
                    className={`px-4 py-1.5 rounded-lg text-[9px] font-bold uppercase tracking-widest transition-all cursor-pointer ${!isCodeView ? 'bg-[var(--color-wardian-accent)] text-black' : 'text-muted-neutral hover:text-[var(--color-wardian-text)] hover:bg-white/5'}`}
                >Visual</button>
                <button 
                    onClick={() => setIsCodeView(true)}
                    className={`px-4 py-1.5 rounded-lg text-[9px] font-bold uppercase tracking-widest transition-all cursor-pointer ${isCodeView ? 'bg-[var(--color-wardian-accent)] text-black' : 'text-muted-neutral hover:text-[var(--color-wardian-text)] hover:bg-white/5'}`}
                >JSON</button>
            </div>
            {!isCodeView && (
                <button 
                    onClick={() => addProperty(schemaObj)}
                    className="text-[9px] font-bold text-[var(--color-wardian-accent)] uppercase tracking-widest hover:bg-[var(--color-wardian-accent)]/10 px-4 py-1.5 rounded-lg transition-all flex items-center gap-1.5 border border-transparent hover:border-[var(--color-wardian-accent)]/30"
                >
                    <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="3" d="M12 4v16m8-8H4"></path></svg>
                    Add Property
                </button>
            )}
        </div>

        {isCodeView ? (
            <div className="rounded-xl overflow-hidden">
              <RenderableInput 
                  multiline
                  nodeId={nodeId}
                  value={value}
                  onChange={onChange}
              />
            </div>
        ) : (
            <div className="bg-[var(--color-wardian-card)] rounded-2xl p-4 border border-wardian-border shadow-inner min-h-[100px]">
                {renderProperties(schemaObj)}
            </div>
        )}
    </div>
  );
};
