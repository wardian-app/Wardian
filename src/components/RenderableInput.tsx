import { useState, useRef, memo, ReactNode, useCallback } from 'react';
import { VariablePill } from './VariablePill';
import { ContextMenu, ContextMenuItem } from './ContextMenu';
import { useUpstreamContext, getDeepKeys } from '../features/workflows/useUpstreamContext';
import { BLOCK_LIBRARY } from '../features/workflows/blockLibrary';

interface RenderableInputProps {
  value: string;
  onChange: (val: string) => void;
  placeholder?: string;
  className?: string;
  multiline?: boolean;
  compact?: boolean;
  nodeId?: string;
}

export const RenderableInput = memo(({ 
  value, 
  onChange, 
  placeholder, 
  className, 
  multiline = false, 
  compact = false,
  nodeId 
}: RenderableInputProps) => {
  const [isFocused, setIsFocused] = useState(false);
  const [internalValue, setInternalValue] = useState(value);
  const [menuPos, setMenuPos] = useState<{ x: number; y: number } | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement | HTMLTextAreaElement>(null);

  const upstream = useUpstreamContext(nodeId || "");

  // Regex to find Handlebars: {{something}}
  const HB_REGEX = /\{\{([^}]+)\}\}/g;

  const renderContent = () => {
    if (!value) return <span className="text-[var(--color-wardian-text-muted)] opacity-50 italic">{placeholder}</span>;

    const elements: (string | ReactNode)[] = [];
    let lastIndex = 0;
    let match;

    HB_REGEX.lastIndex = 0;
    while ((match = HB_REGEX.exec(value)) !== null) {
      if (match.index > lastIndex) {
        elements.push(value.substring(lastIndex, match.index));
      }
      
      const path = match[1];
      elements.push(<VariablePill key={`${match.index}-${path}`} path={path} />);
      lastIndex = HB_REGEX.lastIndex;
    }

    if (lastIndex < value.length) {
      elements.push(value.substring(lastIndex));
    }

    return elements;
  };

  const padding = compact ? 'p-1.5 px-2' : 'p-2.5 px-3';
  const minHeight = compact ? 'min-h-[28px]' : 'min-h-[40px]';
  const fontSize = compact ? 'text-[11px]' : 'text-[14px]';

  const handleContextMenu = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    setMenuPos({ x: e.clientX, y: e.clientY });
  }, []);

  const insertVariable = (path: string) => {
    const tag = `{{${path}}}`;
    setIsFocused(true); // Ensure we are in edit mode
    
    // Use a small delay to ensure the DOM has updated if we were transitioning back from blur
    setTimeout(() => {
      const target = inputRef.current;
      if (!target) {
        onChange(value + tag);
        return;
      }

      const start = target.selectionStart || 0;
      const end = target.selectionEnd || 0;
      const newVal = value.substring(0, start) + tag + value.substring(end);
      setInternalValue(newVal);
      onChange(newVal);
      
      target.focus();
      const newPos = start + tag.length;
      target.setSelectionRange(newPos, newPos);
    }, 10);
  };

  const mapKeysToItems = useCallback((keys: any[], basePath: string): ContextMenuItem[] => {
    return keys.map(k => ({
      label: k.label,
      onClick: k.children ? undefined : () => insertVariable(`${basePath}.${k.path}`),
      subItems: k.children ? mapKeysToItems(k.children, basePath) : undefined
    }));
  }, [insertVariable]);

  const menuItems: ContextMenuItem[] = [
    { 
      label: 'Copy', 
      onClick: () => navigator.clipboard.writeText(value),
      icon: <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path d="M8 5H6a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2v-1M8 5a2 2 0 002 2h2a2 2 0 002-2M8 5a2 2 0 012-2h2a2 2 0 012 2"></path></svg>
    },
    { 
      label: 'Paste', 
      onClick: async () => {
        const text = await navigator.clipboard.readText();
        insertVariable(text.replace(/[{}]/g, '')); // Crude paste for now
      },
      icon: <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2"></path></svg>
    },
    { divider: true },
    {
      label: 'Insert Variable',
      icon: <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path d="M12 4v16m8-8H4"></path></svg>,
      subItems: [
        ...upstream.currentNodes.map(n => {
          const block = BLOCK_LIBRARY.find(b => b.type === n.type && (n.data?.blockName ? b.name === n.data.blockName : true));
          const keys = getDeepKeys(n);
          return {
            label: `${n.data?.label || n.id} (${block?.name || n.type})`,
            subItems: [
              { label: 'Base Output', onClick: () => insertVariable(`nodes.${n.id}.output`) },
              { divider: true },
              ...mapKeysToItems(keys, `nodes.${n.id}.output`)
            ]
          };
        })
      ]
    },
    {
      label: 'Shared Storage',
      icon: <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path d="M4 7v10c0 2 1 3 3 3h10c2 0 3-1 3-3V7c0-2-1-3-3-3H7c-2 0-3 1-3 3zM9 12h6M9 16h6"></path></svg>,
      subItems: [
        { label: 'Base Path (storage.*)', onClick: () => insertVariable('storage.') },
        { divider: true },
        { label: 'Pick Key...', onClick: () => insertVariable('storage.CHANGE_ME') }
      ]
    }
  ];

  return (
    <div 
      ref={containerRef}
      onContextMenu={handleContextMenu}
      className={`relative rounded-xl border transition-all overflow-hidden cursor-text ${isFocused ? 'ring-2 ring-[var(--color-wardian-accent)] border-[var(--color-wardian-accent)] bg-[var(--color-wardian-bg)]' : 'border-wardian-border bg-[color-mix(in_srgb,var(--color-wardian-bg),black_5%)]'} ${className}`}
      onClick={() => {
        if (!isFocused) {
          setInternalValue(value);
          setIsFocused(true);
        }
      }}
    >
      {isFocused ? (
        multiline ? (
          <textarea
            ref={inputRef as React.RefObject<HTMLTextAreaElement>}
            autoFocus
            className={`w-full ${padding} bg-transparent ${fontSize} text-[var(--color-wardian-text)] outline-none resize-y ${compact ? 'min-h-[60px]' : 'min-h-[120px]'} font-mono no-scrollbar`}
            value={internalValue}
            onChange={(e) => {
              setInternalValue(e.target.value);
              onChange(e.target.value);
            }}
            onBlur={() => setIsFocused(false)}
            placeholder={placeholder}
          />
        ) : (
          <input
            ref={inputRef as React.RefObject<HTMLInputElement>}
            autoFocus
            className={`w-full ${padding} bg-transparent ${fontSize} text-[var(--color-wardian-text)] outline-none font-mono`}
            value={internalValue}
            onChange={(e) => {
              setInternalValue(e.target.value);
              onChange(e.target.value);
            }}
            onBlur={() => setIsFocused(false)}
            placeholder={placeholder}
          />
        )
      ) : (
        <div className={`w-full ${padding} ${fontSize} text-[var(--color-wardian-text)] whitespace-pre-wrap break-all flex flex-wrap items-center gap-x-1 gap-y-1.5 transition-opacity duration-200 ${minHeight} ${multiline ? 'items-start content-start' : ''}`}>
          {renderContent()}
        </div>
      )}

      {menuPos && (
        <ContextMenu 
          x={menuPos.x} 
          y={menuPos.y} 
          items={menuItems} 
          onClose={() => setMenuPos(null)} 
        />
      )}
    </div>
  );
});
