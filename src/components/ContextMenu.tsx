import { memo, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { useContextMenuSurface } from './useContextMenuSurface';

export interface ContextMenuItem {
  label?: string;
  icon?: React.ReactNode;
  onClick?: () => void;
  subItems?: ContextMenuItem[];
  danger?: boolean;
  divider?: boolean;
}

interface ContextMenuProps {
  x: number;
  y: number;
  items: ContextMenuItem[];
  onClose: () => void;
}

const MenuItem = memo(({ item, index, onClose }: { item: ContextMenuItem; index: number; onClose: () => void }) => {
  const [activeSubMenu, setActiveSubMenu] = useState(false);
  const [direction, setDirection] = useState<'left' | 'right'>('right');
  const itemRef = useRef<HTMLDivElement>(null);
  const timeoutRef = useRef<number | null>(null);

  if (item.divider) {
    return <div key={`div-${index}`} className="h-px bg-wardian-border my-1" />;
  }

  const hasSubItems = item.subItems && item.subItems.length > 0;

  const handleMouseEnter = () => {
    if (timeoutRef.current) window.clearTimeout(timeoutRef.current);
    
    if (hasSubItems && itemRef.current) {
      const rect = itemRef.current.getBoundingClientRect();
      const wouldOverflow = rect.right + 200 > window.innerWidth;
      setDirection(wouldOverflow ? 'left' : 'right');
    }
    
    setActiveSubMenu(true);
  };

  const handleMouseLeave = () => {
    timeoutRef.current = window.setTimeout(() => {
      setActiveSubMenu(false);
    }, 150);
  };

  return (
    <div 
      ref={itemRef}
      className="relative group"
      onMouseEnter={handleMouseEnter}
      onMouseLeave={handleMouseLeave}
    >
      <button
        onClick={(e) => {
          e.stopPropagation();
          if (item.onClick) {
            item.onClick();
            onClose();
          } else if (hasSubItems) {
            // Toggle for click-based interaction (touch/habit)
            setActiveSubMenu(!activeSubMenu);
          }
        }}
        className={`w-full flex items-center justify-between px-3 py-1.5 text-[11px] font-medium transition-colors hover:bg-white/10
          ${item.danger ? 'text-[var(--color-wardian-error)]' : 'text-[var(--color-wardian-text)]'}`}
      >
        <div className="flex items-center gap-2">
          {item.icon && <span className="opacity-70 group-hover:opacity-100">{item.icon}</span>}
          <span>{item.label}</span>
        </div>
        {hasSubItems && (
          <svg className={`w-3 h-3 opacity-50 transition-transform ${activeSubMenu ? 'rotate-90' : ''}`} fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M9 5l7 7-7 7" />
          </svg>
        )}
      </button>

      {hasSubItems && activeSubMenu && (
        <div 
          className={`absolute ${direction === 'left' ? 'right-full mr-1' : 'left-full ml-1'} top-0 min-w-[200px] bg-[var(--color-wardian-bg)] border border-wardian-border rounded-lg shadow-2xl p-1 z-[1001] backdrop-blur-3xl animate-in fade-in ${direction === 'left' ? 'slide-in-from-right-2' : 'slide-in-from-left-2'} duration-150 ring-1 ring-white/10`}
          onMouseEnter={handleMouseEnter}
        >
          {item.subItems!.map((sub, i) => (
            <MenuItem key={`${sub.label}-${i}`} item={sub} index={i} onClose={onClose} />
          ))}
        </div>
      )}
    </div>
  );
});

export const ContextMenu = memo(({ x, y, items, onClose }: ContextMenuProps) => {
  const { menuRef, style } = useContextMenuSurface<HTMLDivElement>(x, y, onClose);

  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        onClose();
      }
    };
    document.addEventListener('mousedown', handleClickOutside);
    document.addEventListener('wheel', onClose, { passive: true });
    return () => {
      document.removeEventListener('mousedown', handleClickOutside);
      document.removeEventListener('wheel', onClose);
    };
  }, [onClose]);

  return createPortal(
    <div 
      ref={menuRef}
      style={style}
      className="fixed min-w-[200px] bg-[var(--color-wardian-bg)] border border-wardian-border rounded-xl shadow-2xl p-1 z-[9999] backdrop-blur-3xl animate-in zoom-in-95 duration-100 ring-1 ring-white/10"
      onContextMenu={(e) => e.preventDefault()}
    >
      {items.map((item, i) => (
        <MenuItem key={`${item.label}-${i}`} item={item} index={i} onClose={onClose} />
      ))}
    </div>,
    document.body
  );
});
