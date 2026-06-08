import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl } from "@tauri-apps/plugin-opener";
import { render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ChatMarkdown } from "./ChatMarkdown";
import { safeMarkdownUrl } from "./markdownSafety";

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  writeText: vi.fn(),
}));

describe("safeMarkdownUrl", () => {
  it("allows http, https, and file urls while rejecting unsafe schemes", () => {
    expect(safeMarkdownUrl("https://example.test/docs")).toBe("https://example.test/docs");
    expect(safeMarkdownUrl("http://example.test/docs")).toBe("http://example.test/docs");
    expect(safeMarkdownUrl("file:///tmp/AGENTS.md")).toBe("file:///tmp/AGENTS.md");
    expect(safeMarkdownUrl("javascript:alert(1)")).toBeNull();
    expect(safeMarkdownUrl("data:text/html,hello")).toBeNull();
  });
});

describe("ChatMarkdown", () => {
  it("renders email summaries as semantic GFM tables", () => {
    render(
      <ChatMarkdown
        source={[
          "| # | Subject | From | Received |",
          "|---|---------|------|----------|",
          "| 1 | RE: Synthetic compliance question for TEST-123 | Alex Reviewer | 2026-06-06 |",
          "| 8 | | Morgan Coordinator | 2026-06-05 |",
        ].join("\n")}
      />,
    );

    const table = screen.getByRole("table");
    expect(within(table).getByRole("columnheader", { name: "#" })).toBeInTheDocument();
    expect(within(table).getByRole("columnheader", { name: "Subject" })).toBeInTheDocument();
    expect(within(table).getByRole("cell", { name: "RE: Synthetic compliance question for TEST-123" })).toBeInTheDocument();
    expect(within(table).getByRole("cell", { name: "Morgan Coordinator" })).toBeInTheDocument();
  });

  it("preserves table alignment, escaped pipes, empty cells, and overflow wrapping", () => {
    const { container } = render(
      <ChatMarkdown
        source={[
          "| Left | Center | Right | Empty |",
          "|:-----|:------:|------:|-------|",
          "| alpha | escaped \\| pipe | VeryLongSyntheticSubjectWithoutSpacesForWrapping | |",
        ].join("\n")}
      />,
    );

    const wrapper = container.querySelector(".overflow-x-auto");
    expect(wrapper).not.toBeNull();

    const table = screen.getByRole("table");
    expect(within(table).getByRole("columnheader", { name: "Center" })).toHaveStyle({ textAlign: "center" });
    expect(within(table).getByRole("columnheader", { name: "Right" })).toHaveStyle({ textAlign: "right" });
    expect(within(table).getByRole("cell", { name: "escaped | pipe" })).toBeInTheDocument();
    expect(within(table).getByRole("cell", { name: "VeryLongSyntheticSubjectWithoutSpacesForWrapping" })).toHaveClass("break-words");
    expect(table.querySelectorAll("tbody td")[3]).toHaveTextContent("");
  });

  it("renders nested lists and task lists", () => {
    render(
      <ChatMarkdown
        source={[
          "1. Inspect renderer",
          "   - Confirm table support",
          "   - Confirm link safety",
          "2. Add tests",
          "   - [x] Existing markdown behavior",
          "   - [ ] GFM table coverage",
        ].join("\n")}
      />,
    );

    expect(screen.getByText("Inspect renderer")).toBeInTheDocument();
    const topList = screen.getByText("Inspect renderer").closest("ol");
    expect(topList).not.toBeNull();
    const topItems = topList ? within(topList).getAllByRole("listitem", { hidden: true }) : [];
    const inspectItem = screen.getByText("Inspect renderer").closest("li") as HTMLElement;
    const testsItem = screen.getByText("Add tests").closest("li") as HTMLElement;
    expect(inspectItem.parentElement).toBe(topList);
    expect(testsItem.parentElement).toBe(topList);
    expect(topItems).toContain(inspectItem);
    expect(topItems).toContain(testsItem);
    expect(within(inspectItem).getByText("Confirm table support")).toBeInTheDocument();
    expect(within(inspectItem).getByText("Confirm link safety")).toBeInTheDocument();
    expect(within(inspectItem).getByRole("list")).toHaveTextContent("Confirm table support");
    const checkboxes = screen.getAllByRole("checkbox");
    expect(checkboxes[0]).toBeChecked();
    expect(checkboxes[0]).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: "Completed task item" })).toBe(checkboxes[0]);
    expect(checkboxes[1]).not.toBeChecked();
    expect(checkboxes[1]).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: "Incomplete task item" })).toBe(checkboxes[1]);
    expect(within(testsItem).getByText("Existing markdown behavior")).toBeInTheDocument();
    expect(within(testsItem).getByText("GFM table coverage")).toBeInTheDocument();
    expect(checkboxes[0].closest("li")?.parentElement).toBe(within(testsItem).getByRole("list"));
    expect(checkboxes[1].closest("li")?.parentElement).toBe(within(testsItem).getByRole("list"));
  });

  it("renders blockquotes, horizontal rules, and inline GFM", () => {
    const { container } = render(<ChatMarkdown source={["> quoted error", "", "---", "", "Use **bold**, *italic*, ~~old~~, and `code`."].join("\n")} />);

    expect(screen.getByText("quoted error")).toBeInTheDocument();
    expect(container.querySelector("hr")).not.toBeNull();
    expect(screen.getByText("bold").tagName).toBe("STRONG");
    expect(screen.getByText("italic").tagName).toBe("EM");
    expect(screen.getByText("old").tagName).toBe("DEL");
    expect(screen.getByText("code").tagName).toBe("CODE");
  });

  it("renders safe links and unsafe links without clickable anchors", () => {
    render(<ChatMarkdown source="Safe [docs](https://example.test/docs), local [file](file:///tmp/a.md), unsafe [run](javascript:alert), and bare https://example.test/bare." />);

    expect(screen.getByRole("link", { name: "docs" })).toHaveAttribute("href", "https://example.test/docs");
    expect(screen.getByRole("link", { name: "file" })).toHaveAttribute("href", "file:///tmp/a.md");
    expect(screen.getByRole("link", { name: "https://example.test/bare" })).toHaveAttribute("href", "https://example.test/bare");
    expect(screen.queryByRole("link", { name: "run" })).not.toBeInTheDocument();
    expect(screen.getByText("run")).toBeInTheDocument();
  });

  it("opens safe links through the native opener instead of webview navigation", async () => {
    const openUrlMock = vi.mocked(openUrl);
    openUrlMock.mockResolvedValue(undefined);
    render(<ChatMarkdown source="Read the [docs](https://example.test/docs)." />);

    const link = screen.getByRole("link", { name: "docs" });
    const click = new MouseEvent("click", { bubbles: true, cancelable: true });
    link.dispatchEvent(click);

    expect(click.defaultPrevented).toBe(true);
    await waitFor(() => expect(openUrlMock).toHaveBeenCalledWith("https://example.test/docs"));
  });

  it("renders image markdown as a safe placeholder instead of an img element", () => {
    const { container } = render(<ChatMarkdown source="![diagram](https://example.test/diagram.png)" />);

    expect(container.querySelector("img")).toBeNull();
    expect(screen.getByText("diagram")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "https://example.test/diagram.png" })).toHaveAttribute("href", "https://example.test/diagram.png");
  });

  it("keeps code block copy and highlighting behavior", async () => {
    const writeTextMock = vi.mocked(writeText);
    writeTextMock.mockResolvedValue(undefined);

    const { container } = render(<ChatMarkdown source={"```ts\nconst ready = true;\n```"} />);

    expect(screen.getByText("const ready = true;")).toBeInTheDocument();
    expect(container.querySelector('code[data-language="ts"]')).not.toBeNull();
    screen.getByRole("button", { name: "Copy code block" }).click();
    expect(writeTextMock).toHaveBeenCalledWith("const ready = true;");
  });
});
