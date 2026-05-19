import { useState, useCallback, useEffect, useRef } from 'react';
import { useLayoutStore } from '../../store/useLayoutStore';

const SNAP_WEIGHTS = [0.333, 0.5, 0.666];
const SNAP_THRESHOLD = 0.02; // 2% threshold for magnetic snapping
// Entry into stacked mode requires a clearly-larger track than the 2/3 snap,
// so users can still snap to a 2/3-1/3 layout without being forced stacked.
const STACK_THRESHOLD = Math.max(...SNAP_WEIGHTS) + 0.01;

type ResizeKind = 'h' | 'v' | 'stack-exit';

export const useGridResize = (containerRef: React.RefObject<HTMLDivElement | null>) => {
  const { layout, setColumnTracks, setRowHeight, setGridStacked, setPreviousColumnTracks } = useLayoutStore();
  const [resizing, setResizing] = useState<{ type: ResizeKind; index: number } | null>(null);
  const [guidePos, setGuidePos] = useState<number | null>(null);

  const tracksAtStartRef = useRef<number[] | null>(null);
  const lastGlobalWeightRef = useRef<number | null>(null);

  const startResize = useCallback((type: ResizeKind, index: number) => {
    tracksAtStartRef.current = [...useLayoutStore.getState().layout.column_tracks];
    lastGlobalWeightRef.current = null;
    setResizing({ type, index });
  }, []);

  const handleMouseMove = useCallback((e: MouseEvent) => {
    if (!resizing || !containerRef.current) return;

    const container = containerRef.current;
    const rect = container.getBoundingClientRect();

    if (resizing.type === 'h') {
      const mouseX = e.clientX - rect.left;
      let globalWeight = mouseX / container.clientWidth;

      for (const snap of SNAP_WEIGHTS) {
        if (Math.abs(globalWeight - snap) < SNAP_THRESHOLD) {
          globalWeight = snap;
          break;
        }
      }
      globalWeight = Math.max(0, Math.min(1, globalWeight));

      lastGlobalWeightRef.current = globalWeight;
      setGuidePos(globalWeight * container.clientWidth);

      const totalWeight = layout.column_tracks.reduce((a, b) => a + b, 0);
      const normalizedTracks = layout.column_tracks.map(t => t / totalWeight);
      const cumulativeBefore = normalizedTracks.slice(0, resizing.index).reduce((a, b) => a + b, 0);
      const newActiveTrackWeight = Math.max(0, globalWeight - cumulativeBefore);

      if (normalizedTracks[resizing.index + 1] === undefined) return;

      const delta = newActiveTrackWeight - normalizedTracks[resizing.index];
      const neighborNewWeight = Math.max(0, normalizedTracks[resizing.index + 1] - delta);

      const newTracks = [...normalizedTracks];
      newTracks[resizing.index] = newActiveTrackWeight;
      newTracks[resizing.index + 1] = neighborNewWeight;
      setColumnTracks(newTracks);
    } else if (resizing.type === 'stack-exit') {
      const mouseX = e.clientX - rect.left;
      let globalWeight = mouseX / container.clientWidth;

      for (const snap of SNAP_WEIGHTS) {
        if (Math.abs(globalWeight - snap) < SNAP_THRESHOLD) {
          globalWeight = snap;
          break;
        }
      }
      globalWeight = Math.max(0.05, Math.min(0.95, globalWeight));

      lastGlobalWeightRef.current = globalWeight;
      setGuidePos(globalWeight * container.clientWidth);
      // Live preview: render a 2-column split so the stacked cell shrinks with the drag.
      setColumnTracks([globalWeight, 1 - globalWeight]);
    } else {
      const mouseY = e.clientY - rect.top;
      const SNAP_HEIGHTS = [300, 450, 600, 800];
      const HEIGHT_SNAP_THRESHOLD = 20;

      let finalY = mouseY;
      for (const snap of SNAP_HEIGHTS) {
        if (Math.abs(mouseY - snap) < HEIGHT_SNAP_THRESHOLD) {
          finalY = snap;
          break;
        }
      }

      setGuidePos(finalY);

      const rowIdx = resizing.index;
      const calculatedHeight = finalY / (rowIdx + 1);

      setRowHeight(Math.max(300, calculatedHeight));
    }
  }, [resizing, containerRef, layout.column_tracks, setColumnTracks, setRowHeight]);

  const stopResize = useCallback(() => {
    const current = resizing;
    const snapshot = tracksAtStartRef.current;
    const finalWeight = lastGlobalWeightRef.current;

    if (current?.type === 'h') {
      // Read the latest committed tracks from the store (handleMouseMove wrote them).
      const committed = useLayoutStore.getState().layout.column_tracks;
      const total = committed.reduce((a, b) => a + b, 0);
      const normalized = total > 0 ? committed.map(t => t / total) : committed;
      const max = normalized.reduce((m, t) => Math.max(m, t), 0);

      if (max >= STACK_THRESHOLD) {
        // Enter stacked: save the starting layout so the user can return to it.
        if (snapshot && snapshot.length > 0) {
          setPreviousColumnTracks(snapshot);
          setColumnTracks(snapshot); // Restore so we don't keep the tiny-track state.
        }
        setGridStacked(true);
      }
    } else if (current?.type === 'stack-exit') {
      // Any release below the entry threshold means "exit". Restoring the saved
      // pre-stacked layout preserves N-column setups; without one, fall back to
      // a balanced default rather than the (possibly extreme) preview ratio so
      // we can't immediately re-trigger the entry threshold.
      if (finalWeight !== null && finalWeight < STACK_THRESHOLD) {
        const prev = useLayoutStore.getState().previousColumnTracks;
        setColumnTracks(prev && prev.length > 0 ? prev : [0.5, 0.5]);
        setPreviousColumnTracks(null);
        setGridStacked(false);
      } else if (snapshot && snapshot.length > 0) {
        setColumnTracks(snapshot);
      }
    }

    tracksAtStartRef.current = null;
    lastGlobalWeightRef.current = null;
    setResizing(null);
    setGuidePos(null);
  }, [resizing, setColumnTracks, setGridStacked, setPreviousColumnTracks]);

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
