import { beforeEach, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { GardenRecord } from "./GardenRecord";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("../files/useFileResource", () => ({ useFileResource: () => ({ snapshot: null, error: null, retry: vi.fn() }) }));
const actions = { onOpenAgent: vi.fn(), onOpenSkill: vi.fn(), onOpenPath: vi.fn() };
beforeEach(() => { vi.mocked(invoke).mockReset(); vi.clearAllMocks(); });

it("reads skills using the Library-relative path and exposes its canonical action", async () => {
  vi.mocked(invoke).mockResolvedValue("# Planning\nEvidence first.");
  render(<GardenRecord target={{ kind: "skill", id: "skills/dev/planning" }} {...actions} />);
  expect(await screen.findByText("Evidence first.", { selector: "p" })).toBeVisible();
  expect(screen.getByRole("heading", { name: "Skill" })).toBeVisible();
  const source = screen.getByText("Full source").closest("details")!;
  expect(source).not.toHaveAttribute("open");
  fireEvent.click(screen.getByText("Full source"));
  expect(source.querySelector("pre")).toHaveTextContent("# Planning");
  expect(invoke).toHaveBeenCalledWith("read_library_item", { section: "skills", path: "dev/planning" });
  fireEvent.click(screen.getByRole("button", { name: "Open in Library" }));
  expect(actions.onOpenSkill).toHaveBeenCalledWith("skills/dev/planning");
});

it("renders memory scope, evidence and revisions from canonical commands", async () => {
  const memory = { text: "Prefer focused checks", workspace: "/work", kind: "stable", status: "active", revision: 2, revision_id: "rev2", last_verified_at: "2026-09-07", evidence_excerpt: "Run focused verification", sources: [{ source_type: "conversation", locator: "turn-2" }] };
  vi.mocked(invoke).mockImplementation(async (command) => command === "memory_history" ? [memory] : memory);
  render(<GardenRecord target={{ kind: "memory", id: "m1" }} {...actions} />);
  expect(await screen.findByText("Scope")).toBeVisible();
  expect(screen.getByText("/work")).toBeVisible();
  expect(screen.getAllByText("Run focused verification")).toHaveLength(2);
  expect(screen.getByRole("article", { name: "memory record" })).toBeVisible();
  expect(screen.getByRole("heading", { name: "Memory" })).toBeVisible();
  expect(screen.getByText("Sources (1)").closest("details")).not.toHaveAttribute("open");
  fireEvent.click(screen.getByText("Sources (1)"));
  expect(screen.getByText("conversation · turn-2")).toBeVisible();
  expect(document.querySelector('time[datetime="2026-09-07"]')).toHaveAttribute("title", "2026-09-07");
  expect(document.querySelector("time")?.textContent).not.toBe("2026-09-07");
  expect(invoke).toHaveBeenCalledWith("memory_get", { memoryId: "m1" });
  expect(invoke).toHaveBeenCalledWith("memory_history", { memoryId: "m1" });
});

it("does not present an unavailable record as an empty successful read and retries", async () => {
  vi.mocked(invoke).mockRejectedValueOnce(new Error("Permission denied")).mockResolvedValue("Recovered content");
  render(<GardenRecord target={{ kind: "skill", id: "skills/plan" }} {...actions} />);
  expect(await screen.findByRole("alert")).toHaveTextContent("Permission denied");
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  expect(await screen.findByText("Recovered content", { selector: "p" })).toBeVisible();
});

it("renders prose without granting access to linked local files or images and retains exact source", async () => {
  const source = "## Read carefully\n\n[Local notes](./private.md)\n\n![Reference](./private.png)";
  vi.mocked(invoke).mockResolvedValue(source);
  const { container } = render(<GardenRecord target={{ kind: "skill", id: "skills/plan" }} {...actions} />);
  expect(await screen.findByRole("heading", { name: "Read carefully" })).toBeVisible();
  expect(container.querySelector("img")).toBeNull();
  expect(screen.queryByRole("link", { name: "Local notes" })).not.toBeInTheDocument();
  expect(container.querySelector("pre")?.textContent).toBe(source);
});
