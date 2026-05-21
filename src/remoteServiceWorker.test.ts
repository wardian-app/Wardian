import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";

type ListenerMap = Map<string, Array<(event: unknown) => void>>;

function requestKey(request: Request | string): string {
  return typeof request === "string" ? new URL(request, "https://wardian.tailnet.ts.net").href : request.url;
}

function createCaches() {
  const stores = new Map<string, Map<string, Response>>();
  return {
    open: vi.fn(async (name: string) => {
      let store = stores.get(name);
      if (!store) {
        store = new Map();
        stores.set(name, store);
      }
      return {
        addAll: vi.fn(async (urls: string[]) => {
          for (const url of urls) {
            store.set(requestKey(url), new Response(`cached:${url}`));
          }
        }),
        match: vi.fn(async (request: Request | string) => store.get(requestKey(request))?.clone()),
        put: vi.fn(async (request: Request | string, response: Response) => {
          store.set(requestKey(request), response.clone());
        }),
      };
    }),
    keys: vi.fn(async () => [...stores.keys()]),
    delete: vi.fn(async (name: string) => stores.delete(name)),
    match: vi.fn(async (request: Request | string) => {
      for (const store of stores.values()) {
        const response = store.get(requestKey(request));
        if (response) return response.clone();
      }
      return undefined;
    }),
  };
}

function loadRemoteServiceWorker(fetchMock: typeof fetch) {
  const listeners: ListenerMap = new Map();
  const selfScope = {
    location: { origin: "https://wardian.tailnet.ts.net" },
    clients: { claim: vi.fn() },
    skipWaiting: vi.fn(),
    addEventListener: vi.fn((type: string, listener: (event: unknown) => void) => {
      listeners.set(type, [...(listeners.get(type) ?? []), listener]);
    }),
  };
  const caches = createCaches();
  const script = readFileSync(join(process.cwd(), "public", "remote-sw.js"), "utf8");

  new Function("self", "caches", "fetch", "URL", "Response", script)(
    selfScope,
    caches,
    fetchMock,
    URL,
    Response,
  );

  return { listeners, caches };
}

async function dispatchFetch(listener: (event: unknown) => void, request: Request): Promise<Response | undefined> {
  let response: Promise<Response | undefined> | undefined;
  listener({
    request,
    respondWith: vi.fn((value: Promise<Response | undefined>) => {
      response = value;
    }),
  });
  return response;
}

describe("remote service worker", () => {
  it("runtime-caches successful remote asset responses for flaky network reuse", async () => {
    const assetUrl = "https://wardian.tailnet.ts.net/assets/index-abcd.js";
    const fetchMock = vi.fn(async () => new Response("asset-v1", { status: 200 })) as unknown as typeof fetch;
    const { listeners } = loadRemoteServiceWorker(fetchMock);
    const fetchListener = listeners.get("fetch")?.[0];
    expect(fetchListener).toBeDefined();

    const onlineResponse = await dispatchFetch(fetchListener!, new Request(assetUrl));
    expect(await onlineResponse?.text()).toBe("asset-v1");

    vi.mocked(fetchMock).mockRejectedValue(new Error("offline"));
    const cachedResponse = await dispatchFetch(fetchListener!, new Request(assetUrl));

    expect(await cachedResponse?.text()).toBe("asset-v1");
  });
});
