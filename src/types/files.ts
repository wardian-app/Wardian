export type FileResourceKey = `file:${string}` | `artifact:${string}`;

export type FilesSurfaceStateV1 = {
  resource_kind: "file" | "artifact";
  mode: "preview" | "changes" | "draft";
  transient_preview: boolean;
  review_drawer_open: boolean;
  selected_version_id: string | null;
  optional_checkpoint_id: string | null;
};

export type FilesComparisonBaseline =
  | { kind: "saved_file" }
  | { kind: "prompt_checkpoint"; checkpoint_id: string }
  | { kind: "presented_version"; version_id: string }
  | { kind: "previous_presented_version"; version_id: string };

export type FilesSurfaceStateV2 = {
  resource_kind: "file" | "artifact";
  transient_preview: boolean;
  presentation: "rendered" | "editor";
  comparison_open: boolean;
  comparison_layout_preference: "auto" | "unified" | "side_by_side";
  comparison_baseline: FilesComparisonBaseline | null;
  review_drawer_open: boolean;
  selected_version_id: string | null;
  optional_checkpoint_id: string | null;
};

/** Migration boundary for restored V1 Files tabs and current V2 editor state. */
export type FilesSurfaceState = FilesSurfaceStateV1 | FilesSurfaceStateV2;

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

export type CloseFileRendererLeaseRequestV1 = {
  resource_id: string;
  subscription_id: string;
  renderer_lease_id: string;
};

export type PickFileResourceRequestV1 = {
  title: string | null;
};

export type FileRecoveryCleanupV1 = {
  recovery_id: string;
  expected_recovery_revision: number;
};

export type SaveFileResourceTextRequestV1 = {
  resource_id: string;
  subscription_id: string;
  expected_revision: number;
  buffer_base_hash: string;
  text: string;
  recovery_cleanup: FileRecoveryCleanupV1 | null;
};

export type FileResourceSaveResultV1 =
  | { status: "saved"; revision: number; content_hash: string }
  | { status: "unchanged"; revision: number; content_hash: string }
  | { status: "stale_conflict"; revision: number; content_hash: string };

export type PickFileResourceSaveTargetRequestV1 = {
  title: string | null;
  default_name: string | null;
};

export type SaveTargetGrantV1 = {
  schema: 1;
  save_target_grant_id: string;
  selected_path: string;
};

export type SaveFileResourceAsTextRequestV1 = {
  save_target_grant_id: string;
  text: string;
};

export type FileResourceSaveAsResultV1 = {
  schema: 1;
  capability_id: string;
  canonical_path: string;
  resource_id: string;
  content_hash: string;
};

export type CheckpointFileRecoveryRequestV1 = {
  recovery_id: string | null;
  expected_recovery_revision: number | null;
  resource_id: string;
  subscription_id: string;
  base_content_hash: string;
  resource_key: string;
  base: string;
  buffer: string;
};

export type FileRecoveryCheckpointV1 = {
  schema: 1;
  recovery_id: string;
  resource_key: string;
  base_content_hash: string;
  base_opaque_revision: string;
  recovery_revision: number;
  created_at_ms: number;
  updated_at_ms: number;
  file_authorization_error: FileResourceErrorV1 | null;
};

export type FileRecoverySummaryV1 = Omit<
  FileRecoveryCheckpointV1,
  "file_authorization_error"
> & {
  display_name: string;
  extension: string | null;
  mime_type: string;
};

export type FileRecoveryV1 = FileRecoverySummaryV1 & {
  base: string;
  buffer: string;
};

export type GetFileRecoveryRequestV1 = {
  recovery_id: string;
  resource_key: string;
};

export type ListFileRecoveriesRequestV1 = {
  resource_key: string;
};

export type DiscardFileRecoveryRequestV1 = {
  recovery_id: string;
  expected_recovery_revision: number;
  resource_key: string;
};

export type MergeFileRecoveryRequestV1 = {
  recovery_id: string;
  expected_recovery_revision: number;
  resource_key: string;
  resource_id: string;
  subscription_id: string;
};

export type FileRecoveryMergeResultV1 =
  | {
      status: "clean";
      recovery_revision: number;
      current_revision: number;
      current_content_hash: string;
      disk_changed: boolean;
      merged_text: string;
    }
  | {
      status: "conflicted";
      recovery_revision: number;
      current_revision: number;
      current_content_hash: string;
      disk_changed: boolean;
      merged_text: string;
    };
