import {
  Component,
  Suspense,
  useMemo,
  useState,
  type ErrorInfo,
  type ReactNode,
} from "react";

import type {
  FileResourceSnapshotV1,
  FilesComparisonBaseline,
  OpenFileResourceRequestV1,
} from "../../types";
import type { FileEditorController } from "./fileEditorController";
import type { FileResourceClient } from "./fileResourceClient";
import type {
  FileContentPresentation,
  FileEditorBufferSnapshot,
  FileRendererPresentationDefinition,
  RendererRegistry,
} from "./rendererRegistry";

type RendererErrorBoundaryProps = {
  children: ReactNode;
  display_name: string;
  reset_token: number;
  on_reset: () => void;
  on_open_with: () => void;
};

type RendererErrorBoundaryState = { error: Error | null };

class RendererErrorBoundary extends Component<
  RendererErrorBoundaryProps,
  RendererErrorBoundaryState
> {
  state: RendererErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): RendererErrorBoundaryState {
    return { error };
  }

  componentDidCatch(_error: Error, _info: ErrorInfo) {
    // The resource-local recovery UI is the intended reporting boundary.
  }

  componentDidUpdate(previous: RendererErrorBoundaryProps) {
    if (previous.reset_token !== this.props.reset_token && this.state.error) {
      this.setState({ error: null });
    }
  }

  render() {
    if (this.state.error) {
      return (
        <section className="files-resource-state files-renderer-error" role="alert">
          <h2>Renderer could not display {this.props.display_name}</h2>
          <p>{this.state.error.message}</p>
          <div className="files-resource-actions">
            <button type="button" onClick={this.props.on_reset}>Reset Renderer</button>
            <button type="button" onClick={this.props.on_open_with}>Open With</button>
          </div>
        </section>
      );
    }
    return this.props.children;
  }
}

type PresentationLayerProps = {
  presentation: FileContentPresentation;
  definition: FileRendererPresentationDefinition;
  active: boolean;
  reset_token: number;
  snapshot: FileResourceSnapshotV1;
  client: FileResourceClient;
  lifecycle: { visible: boolean };
  surface_id: string;
  editor_controller: FileEditorController | null;
  buffer_snapshot: FileEditorBufferSnapshot | null;
  editor_language: string | null;
  comparison_baseline: FilesComparisonBaseline | null;
  resource_request: OpenFileResourceRequestV1;
  on_reset: () => void;
  on_open_file: (path: string) => Promise<void> | void;
  on_open_with: (path: string) => Promise<void> | void;
  on_reveal: (path: string) => Promise<void> | void;
};

function PresentationLayer({
  presentation,
  definition,
  active,
  reset_token,
  snapshot,
  client,
  lifecycle,
  surface_id,
  editor_controller,
  buffer_snapshot,
  editor_language,
  comparison_baseline,
  resource_request,
  on_reset,
  on_open_file,
  on_open_with,
  on_reveal,
}: PresentationLayerProps) {
  const Renderer = useMemo(() => definition.create_renderer(), [definition, reset_token]);
  const path = snapshot.descriptor.canonical_path;
  return (
    <div
      className="files-presentation-layer"
      data-file-presentation={presentation}
      hidden={!active}
    >
      <RendererErrorBoundary
        key={`${snapshot.resource_id}:${presentation}`}
        display_name={snapshot.descriptor.display_name}
        reset_token={reset_token}
        on_reset={on_reset}
        on_open_with={() => void on_open_with(path)}
      >
        <Suspense fallback={<div className="files-resource-state" role="status">Loading renderer…</div>}>
          <Renderer
            key={reset_token}
            snapshot={snapshot}
            client={client}
            lifecycle={lifecycle}
            surface_id={surface_id}
            editor_controller={editor_controller}
            buffer_snapshot={buffer_snapshot}
            editor_language={editor_language}
            comparison_baseline={comparison_baseline}
            resource_request={resource_request}
            on_open_file={on_open_file}
            on_open_with={on_open_with}
            on_reveal={on_reveal}
          />
        </Suspense>
      </RendererErrorBoundary>
    </div>
  );
}

export type FileContentHostProps = {
  snapshot: FileResourceSnapshotV1;
  client: FileResourceClient;
  lifecycle: { visible: boolean };
  registry: RendererRegistry;
  presentation: FileContentPresentation;
  surface_id: string;
  editor_controller: FileEditorController | null;
  buffer_snapshot: FileEditorBufferSnapshot | null;
  comparison_baseline?: FilesComparisonBaseline | null;
  resource_request?: OpenFileResourceRequestV1;
  on_open_file: (path: string) => Promise<void> | void;
  on_open_with: (path: string) => Promise<void> | void;
  on_reveal: (path: string) => Promise<void> | void;
};

/** Hosts stable per-resource presentations without making presentation a resource mode. */
export function FileContentHost({
  snapshot,
  client,
  lifecycle,
  registry,
  presentation,
  surface_id,
  editor_controller,
  buffer_snapshot,
  comparison_baseline = null,
  resource_request,
  on_open_file,
  on_open_with,
  on_reveal,
}: FileContentHostProps) {
  const [resetTokens, setResetTokens] = useState<Record<FileContentPresentation, number>>({
    rendered: 0,
    editor: 0,
  });
  const renderer = registry.resolve(snapshot.descriptor);
  const activePresentation = presentation === "editor" && renderer.editor
    ? "editor"
    : presentation === "rendered" && renderer.rendered
      ? "rendered"
      : renderer.default_presentation ?? (renderer.editor ? "editor" : "rendered");
  const editorLanguage = renderer.editor_language?.(snapshot.descriptor) ?? null;
  const rendererResourceRequest = resource_request ?? {
    path: snapshot.descriptor.canonical_path,
    agent_id: null,
    user_file_capability_id: null,
  };
  const reset = (target: FileContentPresentation) => {
    setResetTokens((current) => ({ ...current, [target]: current[target] + 1 }));
  };
  return (
    <div className="files-content-host" data-active-presentation={activePresentation}>
      {renderer.rendered ? (
        <PresentationLayer
          presentation="rendered"
          definition={renderer.rendered}
          active={activePresentation === "rendered"}
          reset_token={resetTokens.rendered}
          snapshot={snapshot}
          client={client}
          lifecycle={lifecycle}
          surface_id={surface_id}
          editor_controller={editor_controller}
          buffer_snapshot={buffer_snapshot}
          editor_language={editorLanguage}
          comparison_baseline={comparison_baseline}
          resource_request={rendererResourceRequest}
          on_reset={() => reset("rendered")}
          on_open_file={on_open_file}
          on_open_with={on_open_with}
          on_reveal={on_reveal}
        />
      ) : null}
      {renderer.editor ? (
        <PresentationLayer
          presentation="editor"
          definition={renderer.editor}
          active={activePresentation === "editor"}
          reset_token={resetTokens.editor}
          snapshot={snapshot}
          client={client}
          lifecycle={lifecycle}
          surface_id={surface_id}
          editor_controller={editor_controller}
          buffer_snapshot={buffer_snapshot}
          editor_language={editorLanguage}
          comparison_baseline={comparison_baseline}
          resource_request={rendererResourceRequest}
          on_reset={() => reset("editor")}
          on_open_file={on_open_file}
          on_open_with={on_open_with}
          on_reveal={on_reveal}
        />
      ) : null}
    </div>
  );
}
