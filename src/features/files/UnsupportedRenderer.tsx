import type { FileRendererProps } from "./rendererRegistry";

export default function UnsupportedRenderer({
  snapshot,
  on_open_with,
  on_reveal,
}: FileRendererProps) {
  const { descriptor } = snapshot;
  const liveDocument = descriptor.mime_type === "text/html"
    || descriptor.mime_type === "image/svg+xml";
  const reason = descriptor.unavailable_reason
    ?? (liveDocument ? "live_renderer_not_activated" : "renderer_not_activated");
  return (
    <section className="files-resource-state" role="status" aria-label="Preview unavailable">
      <h2>Preview unavailable</h2>
      <p>{reason}</p>
      <dl className="files-resource-metadata">
        <div><dt>Type</dt><dd>{descriptor.mime_type}</dd></div>
        <div><dt>Size</dt><dd>{descriptor.size_bytes.toLocaleString()} bytes</dd></div>
      </dl>
      <div className="files-resource-actions">
        <button type="button" onClick={() => void on_open_with(descriptor.canonical_path)}>
          Open With
        </button>
        <button type="button" onClick={() => void on_reveal(descriptor.canonical_path)}>
          Reveal
        </button>
      </div>
    </section>
  );
}
