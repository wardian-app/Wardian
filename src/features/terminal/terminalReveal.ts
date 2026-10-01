/** Geometry and backend observed on one side of the renderer's write barrier. */
export type TerminalRevealSample = {
  width: number;
  height: number;
  proposedCols: number;
  proposedRows: number;
  cols: number;
  rows: number;
  backend: "webgl" | "dom";
  canonicalFit: boolean;
};

/** Measures the same content viewport for fit, its cache, and the reveal gate. */
export function terminalViewportSize(container: HTMLDivElement) {
  const rect = container.getBoundingClientRect();
  // A scrollbar shrinks the content box without changing the outer rect.
  return {
    width: container.clientWidth || Math.round(rect.width || 0),
    height: container.clientHeight || Math.round(rect.height || 0),
  };
}

/** Requires unchanged fit metrics and a fitted grid across a settled write. */
export function revealLayoutMatches(
  before: TerminalRevealSample,
  after: TerminalRevealSample,
) {
  return before.width === after.width &&
    before.height === after.height &&
    before.proposedCols === after.proposedCols &&
    before.proposedRows === after.proposedRows &&
    before.backend === after.backend &&
    after.cols === before.cols && after.rows === before.rows &&
    (after.canonicalFit || (after.cols === after.proposedCols && after.rows === after.proposedRows));
}
