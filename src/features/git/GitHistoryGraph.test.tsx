import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { GitHistoryGraph } from "./GitHistoryGraph";

const mockInvoke = vi.mocked(invoke);

const openHistoryRefPicker = () => {
  fireEvent.click(screen.getByRole("button", { name: /History refs:/ }));
};

describe("GitHistoryGraph", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    window.localStorage.clear();
  });

  it("renders compact graph rows with refs while keeping commit metadata out of the row", () => {
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
    expect(within(row).queryByText("HEAD")).not.toBeInTheDocument();
    expect(within(row).queryByText("main")).not.toBeInTheDocument();
    expect(within(row).getByText("origin/main")).toBeInTheDocument();
    expect(within(row).getByText("Merge feature branch")).toBeInTheDocument();
    expect(within(row).queryByText("aaaaaaaa")).not.toBeInTheDocument();
    expect(within(row).queryByText(/Ada Lovelace/)).not.toBeInTheDocument();
    expect(within(row).queryByText(/2026-06-25/)).not.toBeInTheDocument();
  });

  it("shows detailed commit metadata in a history row hover", () => {
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
        ]}
      />,
    );

    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();

    const row = screen.getByTestId("history-graph-row-aaaaaaaa");
    fireEvent.mouseEnter(row);

    const tooltip = screen.getByRole("tooltip");
    expect(row).toHaveAttribute("aria-describedby", "history-graph-tooltip-aaaaaaaa");
    expect(within(tooltip).getByText("Merge feature branch")).toBeInTheDocument();
    expect(within(tooltip).getByText("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")).toBeInTheDocument();
    expect(within(tooltip).getByText("Ada Lovelace")).toBeInTheDocument();
    expect(within(tooltip).getByText("2026-06-25 08:00:00 -0400")).toBeInTheDocument();
    expect(within(tooltip).getByText("HEAD, main, origin/main")).toBeInTheDocument();
    expect(within(tooltip).getByText("bbbbbbbb, cccccccc")).toBeInTheDocument();

    fireEvent.mouseLeave(row);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
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

  it("draws merge parent lanes from the merge point without full-height introduced branches", () => {
    render(
      <GitHistoryGraph
        rootPath="C:/repo"
        branch="main"
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
          {
            hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            message: "First parent",
            author: "Grace Hopper",
            date: "2026-06-24 07:00:00 -0400",
            parent_hashes: ["cccccccccccccccccccccccccccccccccccccccc"],
            refs: [],
          },
          {
            hash: "cccccccccccccccccccccccccccccccccccccccc",
            message: "Second parent",
            author: "Katherine Johnson",
            date: "2026-06-23 06:00:00 -0400",
            parent_hashes: [],
            refs: [],
          },
        ]}
      />,
    );

    const mergeSvg = within(screen.getByTestId("history-graph-row-aaaaaaaa")).getByTestId("history-graph-svg-aaaaaaaa");
    const mergePaths = Array.from(mergeSvg.querySelectorAll("path")).map((path) => path.getAttribute("d"));
    expect(mergePaths).not.toContain("M 22 0 V 22");
    expect(mergePaths).toContain("M 22 11 V 22");

    const firstParentSvg = within(screen.getByTestId("history-graph-row-bbbbbbbb")).getByTestId("history-graph-svg-bbbbbbbb");
    const firstParentPaths = Array.from(firstParentSvg.querySelectorAll("path")).map((path) => path.getAttribute("d"));
    expect(firstParentPaths).toContain("M 22 0 V 11");
    expect(firstParentPaths).not.toContain("M 22 11 V 22");
  });

  it("expands a commit into changed files using the first parent and aligned graph placeholders", async () => {
    const onOpenHistoryFile = vi.fn();
    mockInvoke.mockResolvedValue([
      { path: "src/changed.ts", status: "M" },
      { path: "src/renamed.ts", status: "R" },
    ]);

    render(
      <GitHistoryGraph
        rootPath="C:/repo"
        branch="main"
        upstream="origin/main"
        onOpenHistoryFile={onOpenHistoryFile}
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

    fireEvent.click(screen.getByRole("button", { name: "Open src/changed.ts from aaaaaaaa" }));

    expect(onOpenHistoryFile).toHaveBeenCalledWith(
      expect.objectContaining({ hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }),
      { path: "src/changed.ts", status: "M" },
    );

    onOpenHistoryFile.mockClear();
    const openFileButton = screen.getByRole("button", { name: "Open File for src/changed.ts from aaaaaaaa" });
    expect(openFileButton).not.toHaveClass("absolute");
    expect(openFileButton).toHaveClass("shrink-0");
    fireEvent.click(openFileButton);

    expect(onOpenHistoryFile).toHaveBeenCalledWith(
      expect.objectContaining({ hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }),
      { path: "src/changed.ts", status: "M" },
    );

    onOpenHistoryFile.mockClear();
    fireEvent.contextMenu(changeRow, { clientX: 12, clientY: 24 });
    fireEvent.click(screen.getByRole("button", { name: "Open File" }));

    expect(onOpenHistoryFile).toHaveBeenCalledWith(
      expect.objectContaining({ hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }),
      { path: "src/changed.ts", status: "M" },
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

    expect(screen.queryByRole("button", { name: "View history changes as tree" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "View history changes as list" })).not.toBeInTheDocument();

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

  it("does not expose custom history density controls", () => {
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

    render(
      <GitHistoryGraph rootPath="C:/repo" branch="main" upstream="origin/main" entries={entries} />,
    );

    expect(screen.getByTestId("history-graph-row-aaaaaaaa")).toHaveStyle({ height: "22px" });
    expect(screen.queryByRole("button", { name: "Use detailed history density" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Use tiny history density" })).not.toBeInTheDocument();
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

  it("persists history ref selection and notifies the parent without hiding loaded rows", () => {
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
    const onRefFilterChange = vi.fn();

    let view = render(
      <GitHistoryGraph
        rootPath="C:/repo"
        branch="main"
        upstream="origin/main"
        entries={entries}
        onRefFilterChange={onRefFilterChange}
      />,
    );

    expect(screen.getByText("Current work")).toBeInTheDocument();
    expect(screen.getByText("Upstream base")).toBeInTheDocument();
    expect(screen.getByText("Release tag")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "History refs: Auto" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Filter history to current branch" })).not.toBeInTheDocument();

    openHistoryRefPicker();
    fireEvent.click(screen.getByRole("button", { name: "Upstream" }));

    expect(onRefFilterChange).toHaveBeenLastCalledWith("upstream");
    expect(screen.getByText("Current work")).toBeInTheDocument();
    expect(screen.getByText("Upstream base")).toBeInTheDocument();
    expect(screen.getByText("Release tag")).toBeInTheDocument();

    view.unmount();

    view = render(<GitHistoryGraph rootPath="C:/repo" branch="main" upstream="origin/main" entries={entries} />);

    expect(screen.getByText("Current work")).toBeInTheDocument();
    expect(screen.getByText("Upstream base")).toBeInTheDocument();
    expect(screen.getByText("Release tag")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "History refs: Upstream" })).toBeInTheDocument();

    openHistoryRefPicker();
    fireEvent.click(screen.getByRole("button", { name: "Current Branch" }));

    expect(screen.getByText("Current work")).toBeInTheDocument();
    expect(screen.getByText("Upstream base")).toBeInTheDocument();
    expect(screen.getByText("Release tag")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "History refs: Current Branch" })).toBeInTheDocument();

    openHistoryRefPicker();
    fireEvent.click(screen.getByRole("button", { name: "All" }));

    expect(screen.getByText("Current work")).toBeInTheDocument();
    expect(screen.getByText("Upstream base")).toBeInTheDocument();
    expect(screen.getByText("Release tag")).toBeInTheDocument();

    openHistoryRefPicker();
    fireEvent.click(screen.getByRole("button", { name: "v1.0.0" }));

    expect(screen.getByText("Current work")).toBeInTheDocument();
    expect(screen.getByText("Upstream base")).toBeInTheDocument();
    const taggedRows = screen.getAllByTestId(/history-graph-row-/);
    expect(taggedRows).toHaveLength(3);
    expect(taggedRows[2]).toHaveTextContent("Release tag");
    expect(screen.getByRole("button", { name: "History refs: v1.0.0" })).toBeInTheDocument();
    expect(window.localStorage.getItem("wardian:source-control:history-graph:C:/repo:ref-filter")).toBe("ref:v1.0.0");

    view.unmount();

    render(<GitHistoryGraph rootPath="C:/repo" branch="main" upstream="origin/main" entries={entries} />);

    expect(screen.getByText("Current work")).toBeInTheDocument();
    expect(screen.getByText("Upstream base")).toBeInTheDocument();
    const restoredTaggedRows = screen.getAllByTestId(/history-graph-row-/);
    expect(restoredTaggedRows).toHaveLength(3);
    expect(restoredTaggedRows[2]).toHaveTextContent("Release tag");
    expect(screen.getByRole("button", { name: "History refs: v1.0.0" })).toBeInTheDocument();
  });

  it("toggles ref badges between the active filter and all refs per root", () => {
    const entries = [
      {
        hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        message: "Current tagged work",
        author: "Ada Lovelace",
        date: "2026-06-25 08:00:00 -0400",
        parent_hashes: ["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"],
        refs: ["HEAD", "main", "origin/main", "v1.0.0"],
      },
    ];

    const { unmount } = render(
      <GitHistoryGraph rootPath="C:/repo" branch="main" upstream="origin/main" entries={entries} />,
    );

    openHistoryRefPicker();
    fireEvent.click(screen.getByRole("button", { name: "Current Branch" }));

    const currentRow = screen.getByTestId("history-graph-row-aaaaaaaa");
    expect(within(currentRow).queryByText("HEAD")).not.toBeInTheDocument();
    expect(within(currentRow).queryByText("main")).not.toBeInTheDocument();
    expect(within(currentRow).queryByText("origin/main")).not.toBeInTheDocument();
    expect(within(currentRow).queryByText("v1.0.0")).not.toBeInTheDocument();

    openHistoryRefPicker();
    fireEvent.click(screen.getByRole("button", { name: "Show All Ref Badges" }));

    expect(within(currentRow).getByText("origin/main")).toBeInTheDocument();
    expect(within(currentRow).getByText("v1.0.0")).toBeInTheDocument();
    expect(window.localStorage.getItem("wardian:source-control:history-graph:C:/repo:badge-mode")).toBe("all");

    unmount();

    render(<GitHistoryGraph rootPath="C:/repo" branch="main" upstream="origin/main" entries={entries} />);
    openHistoryRefPicker();
    fireEvent.click(screen.getByRole("button", { name: "Current Branch" }));

    expect(within(screen.getByTestId("history-graph-row-aaaaaaaa")).getByText("v1.0.0")).toBeInTheDocument();
  });

  it("reveals and focuses the current history item from the graph controls", () => {
    const scrollIntoView = vi.fn();
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
      configurable: true,
      value: scrollIntoView,
    });

    render(
      <GitHistoryGraph
        rootPath="C:/repo"
        branch="main"
        upstream="origin/main"
        entries={[
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
            parent_hashes: [],
            refs: ["origin/main"],
          },
        ]}
      />,
    );

    const revealButton = screen.getByRole("button", { name: "Go to Current History Item" });
    const currentRow = screen.getByTestId("history-graph-row-aaaaaaaa");

    expect(currentRow).toHaveAttribute("aria-current", "true");

    fireEvent.click(revealButton);

    expect(scrollIntoView).toHaveBeenCalledWith({ block: "center", inline: "nearest" });
    expect(currentRow).toHaveFocus();

    openHistoryRefPicker();
    fireEvent.click(screen.getByRole("button", { name: "Upstream" }));

    expect(screen.getByRole("button", { name: "Go to Current History Item" })).toBeEnabled();
    expect(screen.getByTestId("history-graph-row-aaaaaaaa")).toHaveTextContent("Current work");
  });

  it("renders a compact load-more row at the end of a paged history graph", () => {
    const onLoadMore = vi.fn();

    render(
      <GitHistoryGraph
        rootPath="C:/repo"
        branch="main"
        upstream="origin/main"
        hasMoreHistory
        isLoadingMoreHistory={false}
        onLoadMoreHistory={onLoadMore}
        entries={[
          {
            hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            message: "Current work",
            author: "Ada Lovelace",
            date: "2026-06-25 08:00:00 -0400",
            parent_hashes: ["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"],
            refs: ["HEAD", "main"],
          },
        ]}
      />,
    );

    const loadMore = screen.getByRole("button", { name: "Load more history commits" });
    expect(loadMore).toHaveTextContent("Load More...");
    expect(loadMore).toHaveStyle({ height: "22px" });
    expect(within(loadMore).getByTestId("history-graph-load-more-placeholder")).toHaveAttribute("width", "22");

    fireEvent.click(loadMore);
    expect(onLoadMore).toHaveBeenCalledTimes(1);
  });

  it("opens VS Code-like context actions for history commits", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    mockInvoke.mockResolvedValue([{ path: "src/changed.ts", status: "M" }]);

    render(
      <GitHistoryGraph
        rootPath="C:/repo"
        branch="main"
        upstream="origin/main"
        entries={[
          {
            hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            message: "Context graph commit",
            author: "Ada Lovelace",
            date: "2026-06-25 08:00:00 -0400",
            parent_hashes: ["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"],
            refs: ["HEAD", "main"],
          },
        ]}
      />,
    );

    fireEvent.contextMenu(screen.getByTestId("history-graph-row-aaaaaaaa"), { clientX: 12, clientY: 24 });

    fireEvent.click(screen.getByRole("button", { name: "Copy Commit ID" }));
    expect(writeText).toHaveBeenCalledWith("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

    fireEvent.contextMenu(screen.getByTestId("history-graph-row-aaaaaaaa"), { clientX: 12, clientY: 24 });
    fireEvent.click(screen.getByRole("button", { name: "Copy Commit Message" }));
    expect(writeText).toHaveBeenCalledWith("Context graph commit");

    fireEvent.contextMenu(screen.getByTestId("history-graph-row-aaaaaaaa"), { clientX: 12, clientY: 24 });
    fireEvent.click(screen.getByRole("button", { name: "View Changes" }));

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("git_commit_changes", {
        cwd: "C:/repo",
        hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        parentHash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      });
    });
    expect(await screen.findByTestId("history-graph-change-row-aaaaaaaa-src/changed.ts")).toBeInTheDocument();
  });

  it("delegates history commit changes from the context menu when a handler is provided", () => {
    const onViewHistoryChanges = vi.fn();

    render(
      <GitHistoryGraph
        rootPath="C:/repo"
        branch="main"
        upstream="origin/main"
        onViewHistoryChanges={onViewHistoryChanges}
        entries={[
          {
            hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            message: "Context graph commit",
            author: "Ada Lovelace",
            date: "2026-06-25 08:00:00 -0400",
            parent_hashes: ["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"],
            refs: ["HEAD", "main"],
          },
        ]}
      />,
    );

    fireEvent.contextMenu(screen.getByTestId("history-graph-row-aaaaaaaa"), { clientX: 12, clientY: 24 });
    fireEvent.click(screen.getByRole("button", { name: "View Changes" }));

    expect(onViewHistoryChanges).toHaveBeenCalledWith(
      expect.objectContaining({ hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }),
    );
    expect(mockInvoke).not.toHaveBeenCalledWith("git_commit_changes", expect.anything());
  });

  it("exposes an inline history commit action without expanding the row", () => {
    const onViewHistoryChanges = vi.fn();

    render(
      <GitHistoryGraph
        rootPath="C:/repo"
        branch="main"
        upstream="origin/main"
        onViewHistoryChanges={onViewHistoryChanges}
        entries={[
          {
            hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            message: "Inline graph commit",
            author: "Ada Lovelace",
            date: "2026-06-25 08:00:00 -0400",
            parent_hashes: ["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"],
            refs: ["HEAD", "main"],
          },
        ]}
      />,
    );

    const row = screen.getByTestId("history-graph-row-aaaaaaaa");

    fireEvent.click(screen.getByRole("button", { name: "View Changes for Inline graph commit" }));

    expect(onViewHistoryChanges).toHaveBeenCalledWith(
      expect.objectContaining({ hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }),
    );
    expect(row).toHaveAttribute("aria-expanded", "false");
    expect(mockInvoke).not.toHaveBeenCalledWith("git_commit_changes", expect.anything());
  });
});
