/// <reference types="vitest/globals" />
import "@testing-library/jest-dom/vitest";

// Polyfill browser APIs not available in jsdom
globalThis.ResizeObserver = class ResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

globalThis.IntersectionObserver = class IntersectionObserver {
  root = null;
  rootMargin = "";
  thresholds = [];
  observe() {}
  unobserve() {}
  disconnect() {}
  takeRecords() { return []; }
} as unknown as typeof IntersectionObserver;

// Mock Tauri API - these modules don't exist outside the Tauri runtime
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(),
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(() => ({
    onMoved: vi.fn(() => Promise.resolve(() => {})),
    onResized: vi.fn(() => Promise.resolve(() => {})),
    minimize: vi.fn(() => Promise.resolve()),
    toggleMaximize: vi.fn(() => Promise.resolve()),
    close: vi.fn(() => Promise.resolve()),
  })),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-notification", () => ({
  isPermissionGranted: vi.fn(async () => false),
  requestPermission: vi.fn(async () => "denied"),
  sendNotification: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(async () => {}),
}));

// Mock xterm since it requires a real DOM canvas
vi.mock("@xterm/xterm", () => ({
  Terminal: vi.fn().mockImplementation(function MockTerminal() {
    return {
    open: vi.fn(),
    write: vi.fn(),
    resize: vi.fn(),
    clear: vi.fn(),
    onData: vi.fn(),
    onBinary: vi.fn(),
    onTitleChange: vi.fn(),
    onResize: vi.fn(),
    onScroll: vi.fn(),
    reset: vi.fn(),
    dispose: vi.fn(),
    focus: vi.fn(),
    attachCustomKeyEventHandler: vi.fn(),
    registerLinkProvider: vi.fn(() => ({ dispose: vi.fn() })),
    selectAll: vi.fn(),
    loadAddon: vi.fn(),
    scrollLines: vi.fn(),
    scrollToBottom: vi.fn(),
    scrollToTop: vi.fn(),
    cols: 80,
    rows: 24,
    };
  }),
}));

vi.mock("@xterm/addon-serialize", () => ({
  SerializeAddon: vi.fn().mockImplementation(function MockSerializeAddon() {
    return {
      serialize: vi.fn(() => ""),
      dispose: vi.fn(),
      activate: vi.fn(),
    };
  }),
}));

vi.mock("@xterm/addon-webgl", () => ({
  WebglAddon: vi.fn().mockImplementation(function MockWebglAddon() {
    return {
      onContextLoss: vi.fn(),
      clearTextureAtlas: vi.fn(),
      dispose: vi.fn(),
    };
  }),
}));

vi.mock("@xterm/headless", () => ({
  Terminal: vi.fn().mockImplementation(function MockHeadlessTerminal() {
    return {
      open: vi.fn(),
      write: vi.fn((_data: string, callback?: () => void) => callback?.()),
      loadAddon: vi.fn(),
      dispose: vi.fn(),
      resize: vi.fn(),
      onData: vi.fn(),
      onBinary: vi.fn(),
      onTitleChange: vi.fn(),
      onResize: vi.fn(),
      onScroll: vi.fn(),
      scrollToTop: vi.fn(),
      buffer: {
        active: {
          cursorX: 0,
          cursorY: 0,
          viewportY: 0,
        },
      },
      options: {},
      cols: 80,
      rows: 24,
    };
  }),
}));

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: vi.fn().mockImplementation(function MockFitAddon() {
    return {
      fit: vi.fn(),
    };
  }),
}));
