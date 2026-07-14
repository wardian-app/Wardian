import { By, Key, until } from "selenium-webdriver";

const DEFAULT_TIMEOUT_MS = 20_000;

function attributeSelector(name, value) {
  return `[data-${name}=${JSON.stringify(value)}]`;
}

function surfaceSelector(role, surfaceType, resourceKey) {
  return `[role=${JSON.stringify(role)}]${attributeSelector("surface-type", surfaceType)}`
    + (resourceKey === undefined ? "" : attributeSelector("resource-key", resourceKey));
}

async function invokeTauri(driver, command, args = {}) {
  return await driver.executeAsyncScript((cmd, payload, done) => {
    window.__TAURI_INTERNALS__.invoke(cmd, payload).then(
      (value) => done({ ok: true, value }),
      (error) => done({ ok: false, error: String(error) }),
    );
  }, command, args);
}

async function selectAgentResource(driver, resourceKey, timeoutMs) {
  const response = await invokeTauri(driver, "list_agents");
  if (!response?.ok) throw new Error(`list_agents failed: ${response?.error ?? "unknown error"}`);
  const agent = response.value.find((candidate) => candidate.session_id === resourceKey);
  if (!agent) throw new Error(`agent resource ${resourceKey} is not available`);
  const row = await driver.wait(
    until.elementLocated(By.css(`[aria-label=${JSON.stringify(`Agent ${agent.session_name}`)}]`)),
    timeoutMs,
  );
  await driver.wait(until.elementIsVisible(row), timeoutMs);
  if (await row.getAttribute("data-selected") !== "true") {
    await row.click();
    await driver.wait(async () => await row.getAttribute("data-selected") === "true", timeoutMs);
  }
}

function normalizeOpenArguments(resourceKey, options) {
  if (resourceKey !== null && typeof resourceKey === "object") {
    return { resourceKey: undefined, options: resourceKey };
  }
  return {
    resourceKey: resourceKey ?? undefined,
    options: options ?? {},
  };
}

function normalizeSelectionOptions(options) {
  if (typeof options === "number") {
    return { timeoutMs: options, index: 0 };
  }
  return {
    timeoutMs: options?.timeoutMs ?? DEFAULT_TIMEOUT_MS,
    index: options?.index ?? 0,
  };
}

function resolveIndex(length, index) {
  return index < 0 ? length + index : index;
}

async function displayedElements(driver, selector) {
  const elements = [];
  for (const candidate of await driver.findElements(By.css(selector))) {
    try {
      if (await candidate.isDisplayed()) elements.push(candidate);
    } catch {
      // Dockview can replace a tab node while its group is being activated.
    }
  }
  return elements;
}

async function indexedElement(driver, selector, options) {
  const { timeoutMs, index } = normalizeSelectionOptions(options);
  return await driver.wait(async () => {
    const elements = await displayedElements(driver, selector);
    const resolved = resolveIndex(elements.length, index);
    return resolved >= 0 && resolved < elements.length ? elements[resolved] : false;
  }, timeoutMs);
}

async function displayedElementCount(driver, selector) {
  return (await displayedElements(driver, selector)).length;
}

export async function waitForWorkbenchReady(driver, timeoutMs = DEFAULT_TIMEOUT_MS) {
  const host = await driver.wait(
    until.elementLocated(By.css('[data-testid="workbench-host"]')),
    timeoutMs,
  );
  await driver.wait(until.elementIsVisible(host), timeoutMs);
  return host;
}

export async function visibleWorkbenchTabs(driver) {
  return await driver.executeScript(() => [...document.querySelectorAll(
    '[role="tab"][data-surface-type]',
  )].filter((tab) => {
    const rect = tab.getBoundingClientRect();
    const style = getComputedStyle(tab);
    return rect.width > 0 && rect.height > 0 && style.visibility !== "hidden" && style.display !== "none";
  }).map((tab) => ({
    surface_id: tab.dataset.surfaceId ?? null,
    surface_type: tab.dataset.surfaceType ?? null,
    resource_key: tab.dataset.resourceKey ?? null,
    selected: tab.getAttribute("aria-selected") === "true",
    text: tab.textContent?.trim() ?? "",
  })));
}

export async function workbenchSnapshot(driver) {
  return await driver.executeScript(() => ({
    zoomed_group_id: document.querySelector('[data-testid="workbench-host"]')
      ?.getAttribute("data-zoomed-group-id") ?? null,
    groups: [...document.querySelectorAll('[data-testid="workbench-group"]')].map((group) => ({
      group_id: group.getAttribute("data-group-id"),
      active: group.getAttribute("data-active") === "true",
      tabs: [...group.querySelectorAll('[role="tab"][data-surface-type]')].map((tab) => ({
        surface_id: tab.dataset.surfaceId ?? null,
        surface_type: tab.dataset.surfaceType ?? null,
        resource_key: tab.dataset.resourceKey ?? null,
        selected: tab.getAttribute("aria-selected") === "true",
      })),
    })),
    panels: [...document.querySelectorAll('[data-testid="surface-panel"]')].map((panel) => ({
      surface_id: panel.dataset.surfaceId ?? null,
      surface_type: panel.dataset.surfaceType ?? null,
      resource_key: panel.dataset.resourceKey ?? null,
      visible: panel.getAttribute("aria-hidden") !== "true",
    })),
  }));
}

export async function focusSurfaceTab(
  driver,
  surfaceType,
  resourceKey,
  options,
) {
  const { timeoutMs } = normalizeSelectionOptions(options);
  const selector = surfaceSelector("tab", surfaceType, resourceKey);
  const tab = await indexedElement(driver, selector, options);
  await tab.click();
  return await driver.wait(async () => {
    try {
      const elements = await displayedElements(driver, selector);
      const resolved = resolveIndex(elements.length, normalizeSelectionOptions(options).index);
      const current = resolved >= 0 && resolved < elements.length ? elements[resolved] : null;
      return current !== null && await current.getAttribute("aria-selected") === "true"
        ? current
        : false;
    } catch {
      return false;
    }
  }, timeoutMs);
}

export async function workbenchSurfacePanel(
  driver,
  surfaceType,
  resourceKey,
  options,
) {
  const selector = `[data-testid="surface-panel"]${attributeSelector("surface-type", surfaceType)}`
    + (resourceKey === undefined ? "" : attributeSelector("resource-key", resourceKey));
  const panel = await indexedElement(driver, selector, options);
  return panel;
}

export async function openWorkbenchSurface(
  driver,
  surfaceType,
  resourceKey,
  openOptions,
) {
  if (surfaceType !== null && typeof surfaceType === "object") {
    const request = surfaceType;
    surfaceType = request.surface_type;
    resourceKey = request.resource_key;
    openOptions = {
      ...request,
      toSide: request.toSide ?? request.to_side,
    };
  }
  const normalized = normalizeOpenArguments(resourceKey, openOptions);
  const timeoutMs = normalized.options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  const toSide = normalized.options.toSide === true;
  await waitForWorkbenchReady(driver, timeoutMs);

  if (normalized.resourceKey !== undefined) {
    await selectAgentResource(driver, normalized.resourceKey, timeoutMs);
  }

  await driver.wait(async () => await driver.executeScript(() => {
    const activeGroup = document.querySelector('[data-testid="workbench-group"][data-active="true"]')
      ?? document.querySelector('[data-testid="workbench-group"]');
    const launcher = activeGroup?.querySelector('button[aria-label="Open Surface"]')
      ?? [...(activeGroup?.querySelectorAll("button") ?? [])]
        .find((button) => button.textContent?.trim() === "Open Surface");
    if (!launcher) return false;
    launcher.click();
    return true;
  }), timeoutMs);

  // The tab-strip plus opens the visual Home chooser by default. Native
  // helpers continue through its explicit Browse action so the same code path
  // can select resource-backed surfaces and preserve open-to-side modifiers.
  await driver.wait(async () => await driver.executeScript(() => {
    const palette = document.querySelector('[role="dialog"][aria-label="Open Surface"]');
    if (palette) return true;
    const home = document.querySelector('[role="dialog"][aria-label="Choose a surface"]');
    const browse = [...(home?.querySelectorAll("button") ?? [])]
      .find((button) => button.textContent?.trim() === "Browse all surfaces");
    if (!browse) return false;
    browse.click();
    return true;
  }), timeoutMs);

  const dialog = await driver.wait(
    until.elementLocated(By.css('[role="dialog"][aria-label="Open Surface"]')),
    timeoutMs,
  );
  await driver.wait(until.elementIsVisible(dialog), timeoutMs);

  const option = await dialog.findElement(By.css(
    `[role="option"]${attributeSelector("surface-type", surfaceType)}`,
  ));
  if (toSide) {
    const platform = await driver.executeScript(() => navigator.platform);
    const primaryKey = /Mac|iPhone|iPad/.test(String(platform)) ? Key.COMMAND : Key.CONTROL;
    await driver.actions()
      .keyDown(primaryKey)
      .click(option)
      .keyUp(primaryKey)
      .perform();
  } else {
    await option.click();
  }

  return await focusSurfaceTab(
    driver,
    surfaceType,
    normalized.resourceKey,
    { timeoutMs, index: normalized.options.index ?? (toSide ? -1 : 0) },
  );
}

export async function closeWorkbenchSurface(
  driver,
  surfaceType,
  resourceKey,
  options,
) {
  const { timeoutMs, index } = normalizeSelectionOptions(options);
  const selector = surfaceSelector("tab", surfaceType, resourceKey);
  const countBefore = await displayedElementCount(driver, selector);
  const tab = await focusSurfaceTab(driver, surfaceType, resourceKey, { timeoutMs, index });
  await tab.sendKeys(Key.DELETE);
  await driver.wait(async () => (
    await displayedElementCount(driver, selector)
  ) === countBefore - 1, timeoutMs);
}
