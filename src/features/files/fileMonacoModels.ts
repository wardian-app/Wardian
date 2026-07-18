import type * as Monaco from "monaco-editor";

import type { FileEditorController } from "./fileEditorController";
import { fileDiffDecorations, fileDiffForController } from "./fileDiffModel";

export type MonacoApi = typeof Monaco;

type CanonicalModelLease = {
  model: Monaco.editor.ITextModel;
  references: number;
  controller: FileEditorController;
  applying_controller_text: boolean;
  decoration_ids: string[];
  dispose_content_listener: () => void;
  dispose_controller_listener: () => void;
  dispose_controller_lifetime: () => void;
};

type BaselineModelLease = {
  model: Monaco.editor.ITextModel;
  references: number;
};

const canonicalModels = new Map<string, CanonicalModelLease>();
const baselineModels = new Map<string, BaselineModelLease>();
let monacoPromise: Promise<MonacoApi> | null = null;

/** Loads the single Monaco runtime and Wardian editor worker. */
export function loadFileMonaco(): Promise<MonacoApi> {
  if (!monacoPromise) {
    monacoPromise = Promise.all([
      import("monaco-editor"),
      import("monaco-editor/esm/vs/editor/editor.worker.js?worker"),
    ]).then(([monaco, workerModule]) => {
      const WorkerConstructor = workerModule.default;
      globalThis.MonacoEnvironment = {
        ...globalThis.MonacoEnvironment,
        getWorker: () => new WorkerConstructor(),
      };
      return monaco;
    }).catch((error) => {
      monacoPromise = null;
      throw error;
    });
  }
  return monacoPromise;
}

function updateCanonicalLease(lease: CanonicalModelLease): void {
  const snapshot = lease.controller.getSnapshot();
  if (lease.model.getValue() !== snapshot.working_text) {
    lease.applying_controller_text = true;
    try {
      lease.model.setValue(snapshot.working_text);
    } finally {
      lease.applying_controller_text = false;
    }
  }
  lease.decoration_ids = lease.model.deltaDecorations(
    lease.decoration_ids,
    fileDiffDecorations(fileDiffForController(lease.controller)),
  );
}

function disposeCanonicalLease(key: string, lease: CanonicalModelLease): void {
  if (canonicalModels.get(key) !== lease) return;
  canonicalModels.delete(key);
  lease.dispose_content_listener();
  lease.dispose_controller_listener();
  lease.dispose_controller_lifetime();
  lease.model.deltaDecorations(lease.decoration_ids, []);
  lease.model.dispose();
}

/** Acquires the one writable model owned by a canonical file editor session. */
export function acquireCanonicalFileModel(
  monaco: MonacoApi,
  key: string,
  controller: FileEditorController,
  language: string,
): Monaco.editor.ITextModel {
  const existing = canonicalModels.get(key);
  if (existing) {
    if (existing.controller !== controller) {
      throw new Error("The canonical Monaco model belongs to another editor session.");
    }
    existing.references += 1;
    monaco.editor.setModelLanguage(existing.model, language);
    updateCanonicalLease(existing);
    return existing.model;
  }
  const uri = monaco.Uri.from({
    scheme: "wardian-file",
    path: `/${encodeURIComponent(key)}`,
  });
  const current = controller.getSnapshot();
  const model = monaco.editor.getModel(uri)
    ?? monaco.editor.createModel(current.working_text, language, uri);
  const lease: CanonicalModelLease = {
    model,
    references: 1,
    controller,
    applying_controller_text: false,
    decoration_ids: [],
    dispose_content_listener: () => undefined,
    dispose_controller_listener: () => undefined,
    dispose_controller_lifetime: () => undefined,
  };
  const contentListener = model.onDidChangeContent(() => {
    if (lease.applying_controller_text) return;
    controller.mutate(model.getValue());
  });
  lease.dispose_content_listener = () => contentListener.dispose();
  lease.dispose_controller_listener = controller.subscribe(() => updateCanonicalLease(lease));
  lease.dispose_controller_lifetime = controller.onDispose(() => disposeCanonicalLease(key, lease));
  canonicalModels.set(key, lease);
  updateCanonicalLease(lease);
  return model;
}

export function releaseCanonicalFileModel(
  key: string,
  model: Monaco.editor.ITextModel,
): void {
  const lease = canonicalModels.get(key);
  if (!lease || lease.model !== model) return;
  lease.references = Math.max(0, lease.references - 1);
  // The controller registry, not a presentation, owns the writable model's lifetime.
}

/** Acquires an immutable comparison base keyed by resource, baseline, and exact hash. */
export function acquireFileBaselineModel(
  monaco: MonacoApi,
  key: string,
  text: string,
  language: string,
): Monaco.editor.ITextModel {
  const existing = baselineModels.get(key);
  if (existing) {
    existing.references += 1;
    monaco.editor.setModelLanguage(existing.model, language);
    if (existing.model.getValue() !== text) {
      throw new Error("The comparison baseline key resolved to different content.");
    }
    return existing.model;
  }
  const uri = monaco.Uri.from({
    scheme: "wardian-file-baseline",
    path: `/${encodeURIComponent(key)}`,
  });
  const model = monaco.editor.getModel(uri) ?? monaco.editor.createModel(text, language, uri);
  if (model.getValue() !== text) {
    throw new Error("The comparison baseline key resolved to different content.");
  }
  baselineModels.set(key, { model, references: 1 });
  return model;
}

export function releaseFileBaselineModel(
  key: string,
  model: Monaco.editor.ITextModel,
): void {
  const lease = baselineModels.get(key);
  if (!lease || lease.model !== model) return;
  lease.references = Math.max(0, lease.references - 1);
  if (lease.references > 0) return;
  baselineModels.delete(key);
  model.dispose();
}
