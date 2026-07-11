import {
  calculateTerminalMirrorFit,
  MAX_WEBGL_RENDERERS,
  MAX_XTERM_RENDERERS,
  TerminalRendererBudget,
} from "./terminalRendererBudget";

describe("TerminalRendererBudget", () => {
  it("enforces independent 24 xterm and 12 WebGL LRU limits", () => {
    const budget = new TerminalRendererBudget();
    const evicted: string[] = [];

    for (let index = 0; index <= MAX_XTERM_RENDERERS; index += 1) {
      const id = `xterm-${index}`;
      budget.acquire("xterm", id, () => evicted.push(id));
    }
    for (let index = 0; index <= MAX_WEBGL_RENDERERS; index += 1) {
      const id = `webgl-${index}`;
      budget.acquire("webgl", id, () => evicted.push(id));
    }

    expect(budget.size("xterm")).toBe(MAX_XTERM_RENDERERS);
    expect(budget.size("webgl")).toBe(MAX_WEBGL_RENDERERS);
    expect(evicted).toEqual(["xterm-0", "webgl-0"]);
    expect(budget.has("xterm", "xterm-0")).toBe(false);
    expect(budget.has("webgl", "webgl-0")).toBe(false);
  });

  it("touches a presentation without coupling its xterm and WebGL recency", () => {
    const budget = new TerminalRendererBudget({ xtermLimit: 2, webglLimit: 2 });
    const evicted: string[] = [];
    for (const id of ["a", "b"]) {
      budget.acquire("xterm", id, () => evicted.push(`xterm:${id}`));
      budget.acquire("webgl", id, () => evicted.push(`webgl:${id}`));
    }

    budget.touch("xterm", "a");
    budget.acquire("xterm", "c", () => evicted.push("xterm:c"));
    budget.acquire("webgl", "c", () => evicted.push("webgl:c"));

    expect(evicted).toEqual(["xterm:b", "webgl:a"]);
  });
});

describe("calculateTerminalMirrorFit", () => {
  it("uses normal scale and letterboxes a mirror with extra room", () => {
    expect(calculateTerminalMirrorFit({
      cols: 80,
      rows: 24,
      cellWidth: 10,
      cellHeight: 20,
      viewportWidth: 1_000,
      viewportHeight: 600,
    })).toMatchObject({
      scale: 1,
      offset_x: 100,
      offset_y: 60,
      pan_x: false,
      pan_y: false,
      letterboxed: true,
    });
  });

  it("scales to the readability floor and pans remaining overflow", () => {
    expect(calculateTerminalMirrorFit({
      cols: 160,
      rows: 60,
      cellWidth: 10,
      cellHeight: 20,
      viewportWidth: 700,
      viewportHeight: 500,
      minimumScale: 0.75,
    })).toMatchObject({
      scale: 0.75,
      pan_x: true,
      pan_y: true,
      letterboxed: false,
    });
  });

  it("fits below normal scale without panning when above the floor", () => {
    expect(calculateTerminalMirrorFit({
      cols: 80,
      rows: 24,
      cellWidth: 10,
      cellHeight: 20,
      viewportWidth: 720,
      viewportHeight: 500,
      minimumScale: 0.75,
    })).toMatchObject({
      scale: 0.9,
      pan_x: false,
      pan_y: false,
    });
  });
});
