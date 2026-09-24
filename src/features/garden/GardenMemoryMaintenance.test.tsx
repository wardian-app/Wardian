import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { GardenMemoryMaintenance } from "./GardenMemoryMaintenance";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const plan = {
  schema_version: 1, plan_id: "plan-1", agent_id: "agent-a", idempotency_key: "key-1",
  operations: [
    { op: "revise", memory_id: "memory-1", expected_revision_id: "revision-1", text: "New full text\nSecond line",
      kind: "current", scope: { kind: "workspace", path: "/reviewed/workspace" }, evidence_excerpt: "Fresh evidence",
      add_sources: [{ source_type: "artifact", locator: "note.md", primary: false }] },
    { op: "retire", memory_id: "memory-2", expected_revision_id: "revision-1", reason: "Obsolete checkpoint" },
  ],
};
const before = {
  memory_id: "memory-1", revision_id: "revision-1", text: "First line\n<script>alert('old')</script>",
  kind: "stable", scope: { kind: "agent" }, evidence_excerpt: "Original evidence",
  sources: [{ source_type: "conversation", locator: "turn-1", primary: true }],
};
const after = {
  ...before, revision_id: null, text: "New full text\nSecond line", kind: "current",
  scope: { kind: "workspace", path: "/reviewed/workspace" }, evidence_excerpt: "Fresh evidence",
  sources: [...before.sources, { source_type: "artifact", locator: "note.md", primary: false }],
};
const preview = {
  plan_id: "plan-1", agent_id: "agent-a", operation_count: 2, preview_digest: "sha256:all-records",
  changes: [
    { operation_index: 0, op: "revise", before, after, source_additions: [after.sources[1]], absorbed_memory_ids: ["memory-3"] },
    { operation_index: 1, op: "retire", before: { ...before, memory_id: "memory-2" }, after: null, source_additions: [], absorbed_memory_ids: [], reason: "Obsolete checkpoint" },
  ],
  conflicts: [],
};
const receipt = {
  plan_id: "plan-1", agent_id: "agent-a", idempotency_key: "key-1", preview_digest: "sha256:all-records",
  applied_at: "2026-09-24T00:00:00Z", operations: [{ operation_index: 0, memory_id: "memory-1", revision_id: "revision-2" }],
};

function fileWith(text: string): File {
  const file = new File([text], "maintenance.json", { type: "application/json" });
  Object.defineProperty(file, "text", { value: async () => text });
  return file;
}

function setup() {
  const onApplied = vi.fn();
  render(<GardenMemoryMaintenance agentId="agent-a" agentName="Agent A" onApplied={onApplied} />);
  fireEvent.click(screen.getByRole("button", { name: "Maintain memory…" }));
  return { onApplied, dialog: within(screen.getByRole("dialog", { name: "Memory maintenance" })) };
}

async function importPlan(value: unknown) {
  fireEvent.change(screen.getByLabelText("Maintenance plan (.json)"), { target: { files: [fileWith(JSON.stringify(value))] } });
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("memory_maintenance_preview", { plan: value }));
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(invoke).mockImplementation(async (command) => command === "memory_maintenance_preview" ? preview : receipt);
});

describe("Garden memory maintenance", () => {
  it("shows full revision, scope, evidence, sources, absorption and explicit retirement before a click can apply", async () => {
    const { dialog } = setup();
    await importPlan(plan);
    expect(await dialog.findByText("sha256:all-records")).toBeInTheDocument();
    expect(dialog.getByText("agent-a", { selector: "dd" })).toBeInTheDocument();
    expect(dialog.getByText("2", { selector: "dd" })).toBeInTheDocument();
    expect(dialog.getAllByText(/First line/)[0]).toHaveTextContent("<script>alert('old')</script>");
    expect(dialog.getByText(/New full text/)).toHaveTextContent("New full text Second line");
    expect(dialog.getAllByText("stable").length).toBeGreaterThan(0);
    expect(dialog.getByText("current")).toBeInTheDocument();
    expect(dialog.getAllByText("Agent-wide").length).toBeGreaterThan(0);
    expect(dialog.getByText("Workspace: /reviewed/workspace")).toBeInTheDocument();
    expect(dialog.getAllByText("Original evidence").length).toBeGreaterThan(0);
    expect(dialog.getByText("Fresh evidence")).toBeInTheDocument();
    expect(dialog.getAllByText(/turn-1/).length).toBeGreaterThan(0);
    expect(dialog.getAllByText(/note.md/).length).toBeGreaterThan(0);
    expect(dialog.getByText("memory-3", { exact: false })).toBeInTheDocument();
    expect(dialog.getByText("Operation 2: retire — explicit retirement")).toBeInTheDocument();
    expect(dialog.getByText("Obsolete checkpoint")).toBeInTheDocument();
    expect(document.querySelector("script")).toBeNull();
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it("rejects an owner mismatch and oversized import before invoking the backend", async () => {
    const { dialog } = setup();
    fireEvent.change(dialog.getByLabelText("Maintenance plan (.json)"), { target: { files: [fileWith(JSON.stringify({ ...plan, agent_id: "agent-b" }))] } });
    expect(await dialog.findByRole("alert")).toHaveTextContent("Plan owner agent-b does not match selected agent agent-a.");
    expect(invoke).not.toHaveBeenCalled();
    fireEvent.change(dialog.getByLabelText("Maintenance plan (.json)"), { target: { files: [fileWith("x".repeat(1024 * 1024 + 1))] } });
    expect(await dialog.findByRole("alert")).toHaveTextContent("1 MiB import limit");
    expect(invoke).not.toHaveBeenCalled();
  });

  it("rejects overlong text and source locators before preview", async () => {
    const { dialog } = setup();
    for (const operation of [
      { ...plan.operations[0], text: "x".repeat(8193) },
      { ...plan.operations[0], add_sources: [{ source_type: "artifact", locator: "x".repeat(4097), primary: false }] },
    ]) {
      fireEvent.change(dialog.getByLabelText("Maintenance plan (.json)"), {
        target: { files: [fileWith(JSON.stringify({ ...plan, operations: [operation] }))] },
      });
      expect(await dialog.findByRole("alert")).toBeInTheDocument();
    }
    expect(invoke).not.toHaveBeenCalled();
  });

  it("shows conflicts and disables Apply", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({ ...preview, conflicts: [{ operation_index: 1, code: "revision_changed", explanation: "Expected revision is no longer active" }] });
    const { dialog } = setup();
    await importPlan(plan);
    expect(await dialog.findByText(/Expected revision is no longer active/)).toBeInTheDocument();
    expect(dialog.getByRole("button", { name: "Apply reviewed plan…" })).toBeDisabled();
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it("applies only on click and shows the complete receipt", async () => {
    const { dialog, onApplied } = setup();
    await importPlan(plan);
    const apply = await dialog.findByRole("button", { name: "Apply reviewed plan…" });
    expect(invoke).toHaveBeenCalledTimes(1);
    fireEvent.click(apply);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("memory_maintenance_apply", { plan, previewDigest: preview.preview_digest }));
    expect(await dialog.findByRole("status", { name: "Memory maintenance receipt" })).toHaveTextContent("revision-2");
    expect(onApplied).toHaveBeenCalledOnce();
    expect(dialog.queryByRole("button", { name: "Apply reviewed plan…" })).not.toBeInTheDocument();
  });

  it("shows a declined apply and requires a new preview before retry", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "memory_maintenance_preview") return preview;
      throw new Error("Native confirmation declined");
    });
    const { dialog, onApplied } = setup();
    await importPlan(plan);
    fireEvent.click(await dialog.findByRole("button", { name: "Apply reviewed plan…" }));
    expect(await dialog.findByRole("alert")).toHaveTextContent("Native confirmation declined");
    expect(dialog.queryByRole("button", { name: "Apply reviewed plan…" })).not.toBeInTheDocument();
    expect(dialog.getByRole("button", { name: "Preview again" })).toBeEnabled();
    expect(onApplied).not.toHaveBeenCalled();
  });

  it("looks up an uncertain receipt without replaying Apply", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "memory_maintenance_preview") return preview;
      if (command === "memory_maintenance_receipt") return receipt;
      throw new Error("Apply response lost");
    });
    const { dialog, onApplied } = setup();
    await importPlan(plan);
    fireEvent.click(await dialog.findByRole("button", { name: "Apply reviewed plan…" }));
    expect(await dialog.findByRole("alert")).toHaveTextContent("Apply response lost");
    fireEvent.click(dialog.getByRole("button", { name: "Check apply receipt" }));
    expect(await dialog.findByRole("status", { name: "Memory maintenance receipt" })).toHaveTextContent("revision-2");
    expect(invoke).toHaveBeenCalledWith("memory_maintenance_receipt", { agentId: "agent-a", idempotencyKey: "key-1" });
    expect(invoke).toHaveBeenCalledTimes(3);
    expect(onApplied).toHaveBeenCalledOnce();
  });
});
