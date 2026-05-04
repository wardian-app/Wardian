import { render, screen } from "@testing-library/react";

const reactDomMock = vi.hoisted(() => {
  const renderRoot = vi.fn();
  return {
    createRoot: vi.fn(() => ({ render: renderRoot })),
    renderRoot,
  };
});

vi.mock("react-dom/client", () => ({
  default: {
    createRoot: reactDomMock.createRoot,
  },
  createRoot: reactDomMock.createRoot,
}));

vi.mock("./views/App", () => ({
  default: () => <div data-testid="app-root" />,
}));

vi.mock("./components/ConfirmDialog", () => ({
  ConfirmProvider: ({ children }: { children: React.ReactNode }) => (
    <div data-testid="confirm-provider">{children}</div>
  ),
}));

describe("main", () => {
  beforeEach(() => {
    vi.resetModules();
    reactDomMock.createRoot.mockClear();
    reactDomMock.renderRoot.mockClear();
    document.body.innerHTML = '<div id="root"></div>';
  });

  it("mounts the app inside the confirm provider", async () => {
    await import("./main");

    expect(reactDomMock.createRoot).toHaveBeenCalledWith(document.getElementById("root"));
    expect(reactDomMock.renderRoot).toHaveBeenCalledTimes(1);

    render(reactDomMock.renderRoot.mock.calls[0][0]);
    expect(screen.getByTestId("confirm-provider")).toContainElement(screen.getByTestId("app-root"));
  });
});
