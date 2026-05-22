import { afterEach, describe, expect, it, vi } from "vitest";
import { registerServiceWorker } from "./registerServiceWorker";

const originalServiceWorker = Object.getOwnPropertyDescriptor(navigator, "serviceWorker");

function setServiceWorker(value: Pick<ServiceWorkerContainer, "register"> | undefined) {
  Object.defineProperty(navigator, "serviceWorker", {
    configurable: true,
    value,
  });
}

function removeServiceWorker() {
  Reflect.deleteProperty(navigator, "serviceWorker");
}

describe("registerServiceWorker", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    if (originalServiceWorker) {
      Object.defineProperty(navigator, "serviceWorker", originalServiceWorker);
    } else {
      Reflect.deleteProperty(navigator, "serviceWorker");
    }
  });

  it("registers the remote app-shell service worker on window load", async () => {
    const register = vi.fn().mockResolvedValue(undefined);
    setServiceWorker({ register });

    let loadHandler: EventListener | undefined;
    vi.spyOn(window, "addEventListener").mockImplementation((type, listener) => {
      if (type === "load" && typeof listener === "function") {
        loadHandler = listener;
      }
    });

    registerServiceWorker();
    loadHandler?.(new Event("load"));

    await vi.waitFor(() => {
      expect(register).toHaveBeenCalledWith("/remote-sw.js", { scope: "/remote" });
    });
  });

  it("does not attach a load handler when service workers are unavailable", () => {
    removeServiceWorker();
    const addEventListener = vi.spyOn(window, "addEventListener");

    registerServiceWorker();

    expect(addEventListener).not.toHaveBeenCalledWith("load", expect.any(Function));
  });
});
