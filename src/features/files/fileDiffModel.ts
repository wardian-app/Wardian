import type * as Monaco from "monaco-editor";
import type { FileEditorController } from "./fileEditorController";

export type FileLineChangeKind = "added" | "modified" | "deleted";

export type FileLineChange = Readonly<{
  kind: FileLineChangeKind;
  original_start_line: number | null;
  original_end_line: number | null;
  modified_start_line: number;
  modified_end_line: number;
}>;

export type FileDiffSummary = Readonly<{
  regions: number;
  added_lines: number;
  modified_lines: number;
  deleted_lines: number;
}>;

export type FileDiffModel = Readonly<{
  changes: readonly FileLineChange[];
  summary: FileDiffSummary;
}>;

type LineOperation = Readonly<{
  kind: "equal" | "insert" | "delete";
  line: string;
}>;

const LCS_CELL_LIMIT = 40_000;
const controllerDiffs = new WeakMap<FileEditorController, {
  base_hash: string | null;
  base_revision: number | null;
  buffer_generation: number;
  diff: FileDiffModel;
}>();

function lines(text: string): string[] {
  return text.split(/\r\n|\r|\n/);
}

function appendLcs(
  original: readonly string[],
  modified: readonly string[],
  originalStart: number,
  originalEnd: number,
  modifiedStart: number,
  modifiedEnd: number,
  operations: LineOperation[],
): void {
  const originalLength = originalEnd - originalStart;
  const modifiedLength = modifiedEnd - modifiedStart;
  const width = modifiedLength + 1;
  const table = new Uint32Array((originalLength + 1) * width);
  for (let originalIndex = originalLength - 1; originalIndex >= 0; originalIndex -= 1) {
    for (let modifiedIndex = modifiedLength - 1; modifiedIndex >= 0; modifiedIndex -= 1) {
      const offset = originalIndex * width + modifiedIndex;
      table[offset] = original[originalStart + originalIndex] === modified[modifiedStart + modifiedIndex]
        ? table[(originalIndex + 1) * width + modifiedIndex + 1]! + 1
        : Math.max(
            table[(originalIndex + 1) * width + modifiedIndex]!,
            table[originalIndex * width + modifiedIndex + 1]!,
          );
    }
  }
  let originalIndex = 0;
  let modifiedIndex = 0;
  while (originalIndex < originalLength && modifiedIndex < modifiedLength) {
    const originalLine = original[originalStart + originalIndex]!;
    const modifiedLine = modified[modifiedStart + modifiedIndex]!;
    if (originalLine === modifiedLine) {
      operations.push({ kind: "equal", line: originalLine });
      originalIndex += 1;
      modifiedIndex += 1;
    } else if (
      table[(originalIndex + 1) * width + modifiedIndex]!
      >= table[originalIndex * width + modifiedIndex + 1]!
    ) {
      operations.push({ kind: "delete", line: originalLine });
      originalIndex += 1;
    } else {
      operations.push({ kind: "insert", line: modifiedLine });
      modifiedIndex += 1;
    }
  }
  while (originalIndex < originalLength) {
    operations.push({ kind: "delete", line: original[originalStart + originalIndex]! });
    originalIndex += 1;
  }
  while (modifiedIndex < modifiedLength) {
    operations.push({ kind: "insert", line: modified[modifiedStart + modifiedIndex]! });
    modifiedIndex += 1;
  }
}

function uniqueAnchors(
  original: readonly string[],
  modified: readonly string[],
  originalStart: number,
  originalEnd: number,
  modifiedStart: number,
  modifiedEnd: number,
): Array<readonly [number, number]> {
  const originalOccurrences = new Map<string, { count: number; index: number }>();
  const modifiedOccurrences = new Map<string, { count: number; index: number }>();
  for (let index = originalStart; index < originalEnd; index += 1) {
    const line = original[index]!;
    const occurrence = originalOccurrences.get(line);
    originalOccurrences.set(line, occurrence
      ? { count: occurrence.count + 1, index: occurrence.index }
      : { count: 1, index });
  }
  for (let index = modifiedStart; index < modifiedEnd; index += 1) {
    const line = modified[index]!;
    const occurrence = modifiedOccurrences.get(line);
    modifiedOccurrences.set(line, occurrence
      ? { count: occurrence.count + 1, index: occurrence.index }
      : { count: 1, index });
  }
  const pairs: Array<readonly [number, number]> = [];
  for (const [line, originalOccurrence] of originalOccurrences) {
    const modifiedOccurrence = modifiedOccurrences.get(line);
    if (originalOccurrence.count === 1 && modifiedOccurrence?.count === 1) {
      pairs.push([originalOccurrence.index, modifiedOccurrence.index]);
    }
  }
  pairs.sort((left, right) => left[0] - right[0]);
  if (pairs.length <= 1) return pairs;

  const tails: number[] = [];
  const tailPairIndices: number[] = [];
  const previous = new Int32Array(pairs.length).fill(-1);
  for (let pairIndex = 0; pairIndex < pairs.length; pairIndex += 1) {
    const modifiedIndex = pairs[pairIndex]![1];
    let low = 0;
    let high = tails.length;
    while (low < high) {
      const middle = (low + high) >>> 1;
      if (tails[middle]! < modifiedIndex) low = middle + 1;
      else high = middle;
    }
    tails[low] = modifiedIndex;
    if (low > 0) previous[pairIndex] = tailPairIndices[low - 1]!;
    tailPairIndices[low] = pairIndex;
  }
  const anchors: Array<readonly [number, number]> = [];
  let pairIndex = tailPairIndices[tails.length - 1] ?? -1;
  while (pairIndex >= 0) {
    anchors.push(pairs[pairIndex]!);
    pairIndex = previous[pairIndex]!;
  }
  anchors.reverse();
  return anchors;
}

function appendOperations(
  original: readonly string[],
  modified: readonly string[],
  originalStart: number,
  originalEnd: number,
  modifiedStart: number,
  modifiedEnd: number,
  operations: LineOperation[],
): void {
  while (
    originalStart < originalEnd
    && modifiedStart < modifiedEnd
    && original[originalStart] === modified[modifiedStart]
  ) {
    operations.push({ kind: "equal", line: original[originalStart]! });
    originalStart += 1;
    modifiedStart += 1;
  }
  let sharedSuffix = 0;
  while (
    originalStart < originalEnd - sharedSuffix
    && modifiedStart < modifiedEnd - sharedSuffix
    && original[originalEnd - sharedSuffix - 1] === modified[modifiedEnd - sharedSuffix - 1]
  ) sharedSuffix += 1;
  const innerOriginalEnd = originalEnd - sharedSuffix;
  const innerModifiedEnd = modifiedEnd - sharedSuffix;

  if (originalStart === innerOriginalEnd) {
    for (let index = modifiedStart; index < innerModifiedEnd; index += 1) {
      operations.push({ kind: "insert", line: modified[index]! });
    }
  } else if (modifiedStart === innerModifiedEnd) {
    for (let index = originalStart; index < innerOriginalEnd; index += 1) {
      operations.push({ kind: "delete", line: original[index]! });
    }
  } else {
    const anchors = uniqueAnchors(
      original,
      modified,
      originalStart,
      innerOriginalEnd,
      modifiedStart,
      innerModifiedEnd,
    );
    if (anchors.length) {
      let previousOriginal = originalStart;
      let previousModified = modifiedStart;
      for (const [anchorOriginal, anchorModified] of anchors) {
        appendOperations(
          original,
          modified,
          previousOriginal,
          anchorOriginal,
          previousModified,
          anchorModified,
          operations,
        );
        operations.push({ kind: "equal", line: original[anchorOriginal]! });
        previousOriginal = anchorOriginal + 1;
        previousModified = anchorModified + 1;
      }
      appendOperations(
        original,
        modified,
        previousOriginal,
        innerOriginalEnd,
        previousModified,
        innerModifiedEnd,
        operations,
      );
    } else if (
      (innerOriginalEnd - originalStart) * (innerModifiedEnd - modifiedStart)
      <= LCS_CELL_LIMIT
    ) {
      appendLcs(
        original,
        modified,
        originalStart,
        innerOriginalEnd,
        modifiedStart,
        innerModifiedEnd,
        operations,
      );
    } else {
      for (let index = originalStart; index < innerOriginalEnd; index += 1) {
        operations.push({ kind: "delete", line: original[index]! });
      }
      for (let index = modifiedStart; index < innerModifiedEnd; index += 1) {
        operations.push({ kind: "insert", line: modified[index]! });
      }
    }
  }
  for (let index = sharedSuffix; index > 0; index -= 1) {
    operations.push({ kind: "equal", line: original[originalEnd - index]! });
  }
}

function changeBlocks(operations: readonly LineOperation[], modifiedLineCount: number): FileLineChange[] {
  const changes: FileLineChange[] = [];
  let originalLine = 1;
  let modifiedLine = 1;
  let index = 0;
  while (index < operations.length) {
    if (operations[index]?.kind === "equal") {
      originalLine += 1;
      modifiedLine += 1;
      index += 1;
      continue;
    }
    const originalStart = originalLine;
    const modifiedStart = modifiedLine;
    let deleted = 0;
    let inserted = 0;
    while (index < operations.length && operations[index]?.kind !== "equal") {
      if (operations[index]?.kind === "delete") {
        deleted += 1;
        originalLine += 1;
      } else {
        inserted += 1;
        modifiedLine += 1;
      }
      index += 1;
    }
    const replaced = Math.min(deleted, inserted);
    if (replaced > 0) {
      changes.push(Object.freeze({
        kind: "modified",
        original_start_line: originalStart,
        original_end_line: originalStart + replaced - 1,
        modified_start_line: modifiedStart,
        modified_end_line: modifiedStart + replaced - 1,
      }));
    }
    if (inserted > replaced) {
      changes.push(Object.freeze({
        kind: "added",
        original_start_line: null,
        original_end_line: null,
        modified_start_line: modifiedStart + replaced,
        modified_end_line: modifiedStart + inserted - 1,
      }));
    }
    if (deleted > replaced) {
      const anchor = Math.max(1, Math.min(modifiedLineCount, modifiedStart + replaced));
      changes.push(Object.freeze({
        kind: "deleted",
        original_start_line: originalStart + replaced,
        original_end_line: originalStart + deleted - 1,
        modified_start_line: anchor,
        modified_end_line: anchor,
      }));
    }
  }
  return changes;
}

/** Computes stable line regions for the working buffer against its retained saved-file base. */
export function buildFileDiffModel(originalText: string, modifiedText: string): FileDiffModel {
  if (originalText === modifiedText) {
    return Object.freeze({
      changes: Object.freeze([]),
      summary: Object.freeze({
        regions: 0,
        added_lines: 0,
        modified_lines: 0,
        deleted_lines: 0,
      }),
    });
  }
  const originalLines = lines(originalText);
  const modifiedLines = lines(modifiedText);
  const operations: LineOperation[] = [];
  appendOperations(
    originalLines,
    modifiedLines,
    0,
    originalLines.length,
    0,
    modifiedLines.length,
    operations,
  );
  const changes = Object.freeze(changeBlocks(operations, modifiedLines.length));
  const summary = changes.reduce<FileDiffSummary>((current, change) => {
    const modifiedCount = change.modified_end_line - change.modified_start_line + 1;
    const originalCount = change.original_start_line === null || change.original_end_line === null
      ? 0
      : change.original_end_line - change.original_start_line + 1;
    return Object.freeze({
      regions: current.regions + 1,
      added_lines: current.added_lines + (change.kind === "added" ? modifiedCount : 0),
      modified_lines: current.modified_lines + (change.kind === "modified" ? modifiedCount : 0),
      deleted_lines: current.deleted_lines + (change.kind === "deleted" ? originalCount : 0),
    });
  }, Object.freeze({ regions: 0, added_lines: 0, modified_lines: 0, deleted_lines: 0 }));
  return Object.freeze({ changes, summary });
}

/** Shares one diff computation across every view of a controller generation. */
export function fileDiffForController(controller: FileEditorController): FileDiffModel {
  const snapshot = controller.getSnapshot();
  const cached = controllerDiffs.get(controller);
  if (
    cached
    && cached.base_hash === snapshot.buffer_base_hash
    && cached.base_revision === snapshot.base_revision
    && cached.buffer_generation === snapshot.buffer_generation
  ) return cached.diff;
  const diff = buildFileDiffModel(snapshot.saved_text, snapshot.working_text);
  controllerDiffs.set(controller, {
    base_hash: snapshot.buffer_base_hash,
    base_revision: snapshot.base_revision,
    buffer_generation: snapshot.buffer_generation,
    diff,
  });
  return diff;
}

/** Converts semantic line changes into model-owned Monaco decorations. */
export function fileDiffDecorations(
  diff: FileDiffModel,
): Monaco.editor.IModelDeltaDecoration[] {
  return diff.changes.map((change) => ({
    range: {
      startLineNumber: change.modified_start_line,
      startColumn: 1,
      endLineNumber: change.modified_end_line,
      endColumn: 1,
    },
    options: {
      isWholeLine: true,
      className: `files-diff-${change.kind}-line`,
      linesDecorationsClassName: `files-diff-${change.kind}-gutter`,
      glyphMarginClassName: `files-diff-${change.kind}-glyph`,
      hoverMessage: {
        value: change.kind === "added"
          ? "Added since Saved file"
          : change.kind === "modified"
            ? "Modified since Saved file"
            : "Deleted lines since Saved file",
      },
    },
  }));
}
