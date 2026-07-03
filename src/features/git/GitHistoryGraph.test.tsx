import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { GitHistoryGraph } from "./GitHistoryGraph";

const mockInvoke = vi.mocked(invoke);

describe("GitHistoryGraph", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    window.localStorage.clear();
  });

  it("renders compact graph rows with refs, fixed swimlane metrics, and commit metadata", () => {
    render(
      <GitHistoryGraph
        rootPath="C:/repo"
        branch="main"
        upstream="origin/main"
        entries={[
          {
            hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            message: "Merge feature branch",
            author: "Ada Lovelace",
            date: "2026-06-25 08:00:00 -0400",
            parent_hashes: [
              "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
              "cccccccccccccccccccccccccccccccccccccccc",
            ],
            refs: ["HEAD", "main", "origin/main"],
          },
          {
            hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            message: "Prepare graph",
            author: "Grace Hopper",
            date: "2026-06-24 07:00:00 -0400",
            parent_hashes: ["dddddddddddddddddddddddddddddddddddddddd"],
            refs: [],
          },
        ]}
      />,
    );

    const row = screen.getByTestId("history-graph-row-aaaaaaaa");
    expect(row).toHaveStyle({ height: "22px" });
    expect(within(row).getByTestId("history-graph-svg-aaaaaaaa")).toHaveAttribute("width", "33");
    expect(within(row).getByText("HEAD")).toBeInTheDocument();
    expect(within(row).getByText("main")).toBeInTheDocument();
    expect(within(row).getByText("origin/main")).toBeInTheDocument();
    expect(within(row).getByText("Merge feature branch")).toBeInTheDocument();
    expect(within(row).getByText("aaaaaaaa")).toBeInTheDocument();
    expect(within(row).getByText(/Ada Lovelace/)).toBeInTheDocument();
  });

  it("renders incoming and outgoing divergence nodes with VS Code-style dashed graph markers", () => {
    render(
      <GitHistoryGraph
        rootPath="C:/repo"
        branch="main"
        upstream="origin/main"
        ahead={2}
        behind={1}
        entries={[
          {
            hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            message: "Local branch work",
            author: "Ada Lovelace",
            date: "2026-06-25 08:00:00 -0400",
            parent_hashes: ["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"],
            refs: ["HEAD", "main"],
          },
          {
            hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            message: "Shared base",
            author: "Grace Hopper",
            date: "2026-06-24 07:00:00 -0400",
            parent_hashes: ["cccccccccccccccccccccccccccccccccccccccc"],
            refs: [],
          },
          {
            hash: "dddddddddddddddddddddddddddddddddddddddd",
            message: "Remote branch work",
            author: "Katherine Johnson",
            date: "2026-06-24 09:00:00 -0400",
            parent_hashes: ["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"],
            refs: ["origin/main"],
          },
        ]}
      />,
    );

    const rows = screen.getAllByTestId(/history-graph-row-/);
    expect(rows.map((row) => row.textContent)).toEqual([
      expect.stringContaining("Outgoing Changes"),
      expect.stringContaining("Local branch work"),
      expect.stringContaining("Shared base"),
      expect.stringContaining("Incoming Changes"),
      expect.stringContaining("Remote branch work"),
    ]);

    const outgoingRow = screen.getByTestId("history-graph-row-outgoing-changes");
    const incomingRow = screen.getByTestId("history-graph-row-incoming-changes");
    expect(outgoingRow).not.toHaveAttribute("aria-expanded");
    expect(incomingRow).not.toHaveAttribute("aria-expanded");
    expect(within(outgoingRow).getByText("2 commits")).toBeInTheDocument();
    expect(within(incomingRow).getByText("1 commit")).toBeInTheDocument();
    expect(within(outgoingRow).queryByText("outgoing")).not.toBeInTheDocument();
    expect(within(incomingRow).queryByText("incoming")).not.toBeInTheDocument();
    expect(within(outgoingRow).getByTestId("history-graph-svg-outgoing-changes").querySelector("circle[stroke-dasharray='4,2']")).toBeTruthy();
    expect(within(incomingRow).getByTestId("history-graph-svg-incoming-changes").querySelector("circle[stroke-dasharray='4,2']")).toBeTruthy();

    fireEvent.click(outgoingRow);
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it("expands a commit into changed files using the first parent and aligned graph placeholders", async () => {
    mockInvoke.mockResolvedValue([
      { path: "src/changed.ts", status: "M" },
      { path: "src/renamed.ts", status: "R" },
    ]);

    render(
      <GitHistoryGraph
        rootPath="C:/repo"
        branch="main"
        upstream="origin/main"
        entries={[
          {
            hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            message: "Merge feature branch",
            author: "Ada Lovelace",
            date: "2026-06-25 08:00:00 -0400",
            parent_hashes: [
              "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
              "cccccccccccccccccccccccccccccccccccccccc",
            ],
            refs: ["HEAD", "main"],
          },
        ]}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /Expand Merge feature branch/ }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_changes", {
        cwd: "C:/repo",
        hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        parentHash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      });
    });

    const changeRow = await screen.findByTestId("history-graph-change-row-aaaaaaaa-src/changed.ts");
    expect(changeRow).toHaveStyle({ height: "22px" });
    expect(screen.getByRole("button", { name: "src" })).toHaveAttribute("aria-expanded", "true");
    expect(within(changeRow).getByText("changed.ts")).toBeInTheDocument();
    expect(within(changeRow).getByText("M")).toBeInTheDocument();
    expect(within(changeRow).getByTestId("history-graph-change-placeholder-aaaaaaaa-src/changed.ts")).toHaveAttribute(
      "width",
      "33",
    );
  });

  it("renders expanded commit changes as a collapsible tree and can switch to flat list mode", async () => {
    mockInvoke.mockResolvedValue([
      { path: "README.md", status: "M" },
      { path: "src/components/Button.tsx", status: "M" },
      { path: "src/features/git/GitHistoryGraph.tsx", status: "A" },
    ]);

    render(
      <GitHistoryGraph
        rootPath="C:/repo"
        branch="main"
        upstream="origin/main"
        entries={[
          {
            hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            message: "Structure graph changes",
            author: "Ada Lovelace",
            date: "2026-06-25 08:00:00 -0400",
            parent_hashes: ["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"],
            refs: ["HEAD", "main"],
          },
        ]}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /Expand Structure graph changes/ }));

    const srcFolder = await screen.findByRole("button", { name: "src" });
    expect(srcFolder).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("button", { name: "components" })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("button", { name: "features" })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("button", { name: "git" })).toHaveAttribute("aria-expanded", "true");

    const nestedChange = screen.getByTestId("history-graph-change-row-aaaaaaaa-src/features/git/GitHistoryGraph.tsx");
    expect(within(nestedChange).getByText("GitHistoryGraph.tsx")).toBeInTheDocument();
    expect(within(nestedChange).getByTestId("history-graph-change-placeholder-aaaaaaaa-src/features/git/GitHistoryGraph.tsx")).toHaveAttribute(
      "width",
      "22",
    );
    expect(screen.queryByText("src/features/git/GitHistoryGraph.tsx")).not.toBeInTheDocument();

    fireEvent.click(srcFolder);
    expect(srcFolder).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByTestId("history-graph-change-row-aaaaaaaa-src/features/git/GitHistoryGraph.tsx")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "View history changes as list" }));

    expect(screen.getByText("src/features/git/GitHistoryGraph.tsx")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "src" })).not.toBeInTheDocument();
    expect(window.localStorage.getItem("wardian:source-control:history-graph:C:/repo:change-view-mode")).toBe("list");
  });

  it("switches to tiny density and persists the presentation per root", () => {
    const entries = [
      {
        hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        message: "Merge feature branch",
        author: "Ada Lovelace",
        date: "2026-06-25 08:00:00 -0400",
        parent_hashes: [
          "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
          "cccccccccccccccccccccccccccccccccccccccc",
        ],
        refs: ["HEAD", "main"],
      },
    ];

    const { unmount } = render(
      <GitHistoryGraph rootPath="C:/repo" branch="main" upstream="origin/main" entries={entries} />,
    );

    expect(screen.getByTestId("history-graph-row-aaaaaaaa")).toHaveStyle({ height: "22px" });

    fireEvent.click(screen.getByRole("button", { name: "Use tiny history density" }));

    expect(screen.getByTestId("history-graph-row-aaaaaaaa")).toHaveStyle({ height: "16px" });
    expect(screen.getByTestId("history-graph-svg-aaaaaaaa")).toHaveAttribute("width", "24");
    expect(screen.queryByText(/Ada Lovelace/)).not.toBeInTheDocument();

    unmount();

    const persisted = render(
      <GitHistoryGraph rootPath="C:/repo" branch="main" upstream="origin/main" entries={entries} />,
    );

    expect(screen.getByTestId("history-graph-row-aaaaaaaa")).toHaveStyle({ height: "16px" });

    persisted.unmount();

    render(<GitHistoryGraph rootPath="C:/other-repo" branch="main" upstream="origin/main" entries={entries} />);

    expect(screen.getByTestId("history-graph-row-aaaaaaaa")).toHaveStyle({ height: "22px" });
  });

  it("persists expanded rows per root and collapses them all", async () => {
    mockInvoke.mockResolvedValue([{ path: "src/changed.ts", status: "M" }]);
    const entries = [
      {
        hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        message: "Merge feature branch",
        author: "Ada Lovelace",
        date: "2026-06-25 08:00:00 -0400",
        parent_hashes: ["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"],
        refs: ["HEAD", "main"],
      },
    ];

    const { unmount } = render(
      <GitHistoryGraph rootPath="C:/repo" branch="main" upstream="origin/main" entries={entries} />,
    );

    fireEvent.click(screen.getByRole("button", { name: /Expand Merge feature branch/ }));

    await screen.findByTestId("history-graph-change-row-aaaaaaaa-src/changed.ts");

    unmount();
    mockInvoke.mockClear();

    render(<GitHistoryGraph rootPath="C:/repo" branch="main" upstream="origin/main" entries={entries} />);

    const restoredRow = screen.getByRole("button", { name: /Collapse Merge feature branch/ });
    expect(restoredRow).toHaveAttribute("aria-expanded", "true");

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_changes", {
        cwd: "C:/repo",
        hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        parentHash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      });
    });

    fireEvent.click(screen.getByRole("button", { name: "Collapse all history rows" }));

    expect(screen.getByRole("button", { name: /Expand Merge feature branch/ })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
    expect(screen.queryByTestId("history-graph-change-row-aaaaaaaa-src/changed.ts")).not.toBeInTheDocument();
  });

  it("filters graph rows by refs and persists the filter per root", () => {
    const entries = [
      {
        hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        message: "Current work",
        author: "Ada Lovelace",
        date: "2026-06-25 08:00:00 -0400",
        parent_hashes: ["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"],
        refs: ["HEAD", "main"],
      },
      {
        hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        message: "Upstream base",
        author: "Grace Hopper",
        date: "2026-06-24 07:00:00 -0400",
        parent_hashes: ["cccccccccccccccccccccccccccccccccccccccc"],
        refs: ["origin/main"],
      },
      {
        hash: "cccccccccccccccccccccccccccccccccccccccc",
        message: "Release tag",
        author: "Katherine Johnson",
        date: "2026-06-23 06:00:00 -0400",
        parent_hashes: [],
        refs: ["v1.0.0"],
      },
    ];

    const { unmount } = render(
      <GitHistoryGraph rootPath="C:/repo" branch="main" upstream="origin/main" entries={entries} />,
    );

    expect(screen.getByText("Current work")).toBeInTheDocument();
    expect(screen.getByText("Upstream base")).toBeInTheDocument();
    expect(screen.getByText("Release tag")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Filter history to upstream" }));

    expect(screen.queryByText("Current work")).not.toBeInTheDocument();
    expect(screen.getByText("Upstream base")).toBeInTheDocument();
    expect(screen.queryByText("Release tag")).not.toBeInTheDocument();

    unmount();

    render(<GitHistoryGraph rootPath="C:/repo" branch="main" upstream="origin/main" entries={entries} />);

    expect(screen.queryByText("Current work")).not.toBeInTheDocument();
    expect(screen.getByText("Upstream base")).toBeInTheDocument();
    expect(screen.queryByText("Release tag")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Filter history to current branch" }));

    expect(screen.getByText("Current work")).toBeInTheDocument();
    expect(screen.queryByText("Upstream base")).not.toBeInTheDocument();
    expect(screen.queryByText("Release tag")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Show all history refs" }));

    expect(screen.getByText("Current work")).toBeInTheDocument();
    expect(screen.getByText("Upstream base")).toBeInTheDocument();
    expect(screen.getByText("Release tag")).toBeInTheDocument();
  });
});
