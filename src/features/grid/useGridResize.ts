import { useState, useCallback, useEffect } from 'react';
import { useLayoutStore } from '../../store/useLayoutStore';

const SNAP_WEIGHTS = [0.333, 0.5, 0.666, 1.0];
const SNAP_THRESHOLD = 0.02; // 2% threshold for magnetic snapping
const MIN_TRACK_PX = 400; // Slightly smaller than card min-width to allow tight packing

export const useGridResize = (containerRef: React.RefObject<HTMLDivElement | null>) => {
  const { layout, setColumnTracks, setRowHeight } = useLayoutStore();
  const [resizing, setResizing] = useState<{ type: 'h' | 'v', index: number } | null>(null);
  const [guidePos, setGuidePos] = useState<number | null>(null);

  const startResize = useCallback((type: 'h' | 'v', index: number) => {
    setResizing({ type, index });
  }, []);

  const handleMouseMove = useCallback((e: MouseEvent) => {
    if (!resizing || !containerRef.current) return;

    const rect = containerRef.current.getBoundingClientRect();
    
    if (resizing.type === 'h') {
      const mouseX = e.clientX - rect.left;
      setGuidePos(e.clientX);
      let newWeight = mouseX / rect.width;

      // Snap logic (normalized 0-1)
      for (const snap of SNAP_WEIGHTS) {
        if (Math.abs(newWeight - snap) < SNAP_THRESHOLD) {
          newWeight = snap;
          setGuidePos(rect.left + (snap * rect.width));
          break;
        }
      }

      // Hard clamp for min width
      if (newWeight * rect.width < MIN_TRACK_PX) return;

      const newTracks = [...layout.column_tracks];
      const currentTrackWeight = layout.column_tracks[resizing.index];
      const delta = newWeight - currentTrackWeight;
      
      // Balance with neighbor if it exists (for fluid track system)
      if (newTracks[resizing.index + 1] !== undefined) {
        const neighborNewWeight = newTracks[resizing.index + 1] - delta;
        
        // Ensure neighbor doesn't shrink below minimum
        if (neighborNewWeight * rect.width < MIN_TRACK_PX) return;
        
        newTracks[resizing.index] = newWeight;
        newTracks[resizing.index + 1] = neighborNewWeight;
        setColumnTracks(newTracks);
      }
    } else {
      const mouseY = e.clientY - rect.top;
      setGuidePos(e.clientY);
      
      // We assume synchronized rows, so we just update the global row height
      // based on the dragged card's row position.
      const rowCount = Math.floor(resizing.index / layout.column_tracks.length) + 1;
      const calculatedHeight = mouseY / rowCount;
      
      setRowHeight(Math.max(300, calculatedHeight));
    }
  }, [resizing, containerRef, layout.column_tracks, setColumnTracks, setRowHeight]);

  const stopResize = useCallback(() => {
    setResizing(null);
    setGuidePos(null);
  }, []);

  useEffect(() => {
    if (resizing) {
      window.addEventListener('mousemove', handleMouseMove);
      window.addEventListener('mouseup', stopResize);
      return () => {
        window.removeEventListener('mousemove', handleMouseMove);
        window.removeEventListener('mouseup', stopResize);
      };
    }
  }, [resizing, handleMouseMove, stopResize]);

  return { isResizing: !!resizing, startResize, guidePos, resizeType: resizing?.type };
};
