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

    const container = containerRef.current;
    const rect = container.getBoundingClientRect();
    const totalWeight = layout.column_tracks.reduce((a, b) => a + b, 0);
    // Normalize tracks to 1.0 to handle old [1, 1] relative weights in local storage
    const normalizedTracks = layout.column_tracks.map(t => t / totalWeight);
    
    if (resizing.type === 'h') {
      const mouseX = e.clientX - rect.left;
      // Use clientWidth to exclude vertical scrollbar width from percentage math
      let globalWeight = mouseX / container.clientWidth;

      // Snap global weight
      for (const snap of SNAP_WEIGHTS) {
        if (Math.abs(globalWeight - snap) < SNAP_THRESHOLD) {
          globalWeight = snap;
          break;
        }
      }
      
      setGuidePos(rect.left + (globalWeight * container.clientWidth));

      // Calculate weight of tracks before the active handle
      const cumulativeBefore = normalizedTracks.slice(0, resizing.index).reduce((a, b) => a + b, 0);
      const newActiveTrackWeight = globalWeight - cumulativeBefore;

      // Hard clamp for min width
      if (newActiveTrackWeight * container.clientWidth < MIN_TRACK_PX) return;

      const newTracks = [...normalizedTracks];
      const delta = newActiveTrackWeight - normalizedTracks[resizing.index];
      
      // Balance with immediate right neighbor
      if (newTracks[resizing.index + 1] !== undefined) {
        const neighborNewWeight = newTracks[resizing.index + 1] - delta;
        if (neighborNewWeight * container.clientWidth < MIN_TRACK_PX) return;
        
        newTracks[resizing.index] = newActiveTrackWeight;
        newTracks[resizing.index + 1] = neighborNewWeight;
        setColumnTracks(newTracks);
      }
    } else {
      const mouseY = e.clientY - rect.top;
      // Determine vertical snap
      const SNAP_HEIGHTS = [300, 450, 600, 800];
      const HEIGHT_SNAP_THRESHOLD = 20;
      
      let finalY = mouseY;
      for (const snap of SNAP_HEIGHTS) {
        if (Math.abs(mouseY - snap) < HEIGHT_SNAP_THRESHOLD) {
          finalY = snap;
          break;
        }
      }

      setGuidePos(rect.top + finalY);
      
      // Row height is synchronized across all rows
      const rowIdx = Math.floor(resizing.index / layout.column_tracks.length);
      const calculatedHeight = finalY / (rowIdx + 1);
      
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
