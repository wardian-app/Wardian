export function renderedTerminalRowHeight(root: ParentNode | null | undefined) {
  const rowElements = root?.querySelectorAll<HTMLElement>(".xterm-rows > div");
  const first = rowElements?.[0];
  if (!first) {
    return null;
  }

  const firstRect = first.getBoundingClientRect();
  const secondRect = rowElements?.[1]?.getBoundingClientRect();
  const rowStep = secondRect ? secondRect.top - firstRect.top : 0;
  if (Number.isFinite(rowStep) && rowStep > 0) {
    return rowStep;
  }
  if (Number.isFinite(firstRect.height) && firstRect.height > 0) {
    return firstRect.height;
  }
  return null;
}

export function proposeTerminalRows(
  hostHeight: number,
  xtermCellHeight: number,
  renderedRowHeight: number | null,
) {
  const rowsFromXterm = Math.floor(hostHeight / xtermCellHeight);
  if (!renderedRowHeight || renderedRowHeight <= 0 || renderedRowHeight >= xtermCellHeight) {
    return rowsFromXterm;
  }

  const rowsFromRenderedGeometry = Math.floor(hostHeight / renderedRowHeight);
  const visibleGap = hostHeight - rowsFromXterm * renderedRowHeight;
  return rowsFromRenderedGeometry > rowsFromXterm && visibleGap >= renderedRowHeight
    ? rowsFromRenderedGeometry
    : rowsFromXterm;
}
