export type FileResourceKey = `file:${string}` | `artifact:${string}`;

export type FilesSurfaceStateV1 = {
  resource_kind: "file" | "artifact";
  mode: "preview" | "changes" | "draft";
  transient_preview: boolean;
  review_drawer_open: boolean;
  selected_version_id: string | null;
  optional_checkpoint_id: string | null;
};

export type FileRendererKind = "text" | "markdown" | "image" | "pdf" | "unsupported";

export type FileResourceCapabilitiesV1 = {
  preview: boolean;
  changes: boolean;
  draft: boolean;
  stream: boolean;
};

export type FileContentDescriptorV1 = {
  schema: 1;
  canonical_path: string;
  display_name: string;
  extension: string | null;
  mime_type: string;
  encoding: string | null;
  renderer_kind: FileRendererKind;
  size_bytes: number;
  line_count: number | null;
  content_hash: string;
  modified_at_ms: number;
  capabilities: FileResourceCapabilitiesV1;
  unavailable_reason: string | null;
};

export type FileResourceSnapshotV1 = {
  resource_id: string;
  subscription_id: string;
  revision: number;
  descriptor: FileContentDescriptorV1;
};

export type FileResourceEventV1 = {
  schema: 1;
  resource_id: string;
  revision: number;
  descriptor: FileContentDescriptorV1;
};

export type FileResourceTextV1 = {
  schema: 1;
  resource_id: string;
  revision: number;
  text: string;
};

export type UserFileGrantV1 = {
  schema: 1;
  capability_id: string;
  canonical_path: string;
};

export type FileResourceTicketV1 = {
  schema: 1;
  ticket_id: string;
  url: string;
  resource_id: string;
  revision: number;
  renderer_lease_id: string;
  expires_at_ms: number;
};

export type FileResourceErrorV1 = {
  schema: 1;
  code: string;
  message: string;
};

export type OpenFileResourceRequestV1 = {
  path: string;
  agent_id: string | null;
  user_file_capability_id: string | null;
};

export type CloseFileResourceRequestV1 = {
  subscription_id: string;
};

export type ReadFileResourceTextRequestV1 = {
  resource_id: string;
  subscription_id: string;
  revision: number;
};

export type IssueFileResourceTicketRequestV1 = {
  resource_id: string;
  subscription_id: string;
  revision: number;
  renderer_lease_id: string;
};

export type PickFileResourceRequestV1 = {
  title: string | null;
};
