import {
  Component,
  Suspense,
  useMemo,
  useState,
  type ErrorInfo,
  type ReactNode,
} from "react";

import type { FileResourceSnapshotV1 } from "../../types";
import type { FileResourceClient } from "./fileResourceClient";
import type {
  FilePreviewPresentation,
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

export type FilePreviewProps = {
  snapshot: FileResourceSnapshotV1;
  client: FileResourceClient;
  lifecycle: { visible: boolean };
  registry: RendererRegistry;
  presentation: FilePreviewPresentation;
  on_open_file: (path: string) => Promise<void> | void;
  on_open_with: (path: string) => Promise<void> | void;
  on_reveal: (path: string) => Promise<void> | void;
};

export function FilePreview({
  snapshot,
  client,
  lifecycle,
  registry,
  presentation,
  on_open_file,
  on_open_with,
  on_reveal,
}: FilePreviewProps) {
  const [resetToken, setResetToken] = useState(0);
  const definition = registry.resolve(snapshot.descriptor);
  const activePresentation = presentation === "source" && definition.source
    ? "source"
    : "rendered";
  const createRenderer = activePresentation === "source"
    ? definition.source!.create_renderer
    : definition.create_renderer;
  const Renderer = useMemo(
    () => createRenderer(),
    [createRenderer, resetToken],
  );
  const path = snapshot.descriptor.canonical_path;
  return (
    <RendererErrorBoundary
      key={`${snapshot.resource_id}@${snapshot.revision}:${activePresentation}`}
      display_name={snapshot.descriptor.display_name}
      reset_token={resetToken}
      on_reset={() => setResetToken((value) => value + 1)}
      on_open_with={() => void on_open_with(path)}
    >
      <Suspense fallback={<div className="files-resource-state" role="status">Loading renderer…</div>}>
        <Renderer
          key={resetToken}
          snapshot={snapshot}
          client={client}
          lifecycle={lifecycle}
          on_open_file={on_open_file}
          on_open_with={on_open_with}
          on_reveal={on_reveal}
        />
      </Suspense>
    </RendererErrorBoundary>
  );
}
