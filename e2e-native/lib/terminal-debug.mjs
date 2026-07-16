const TERMINAL_HOST_SELECTOR = '[data-testid="agent-terminal-host"]';

function presentationResolutionTimeoutMessage(scope, sessionId) {
  return `Timed out resolving the terminal presentation for ${scope} ${sessionId}`;
}

/**
 * Resolves the renderer identity from the terminal host owned by one exact
 * Agents card. Runtime session IDs are not renderer identities: one session
 * can be presented in more than one surface at the same time.
 */
export async function resolveAgentTerminalPresentationId(
  driver,
  sessionId,
  timeoutMs = 20_000,
) {
  return await driver.wait(async () => await driver.executeScript((sid, hostSelector) => {
    const card = document.getElementById(`agent-card-${sid}`);
    if (!card) return false;
    const matchingHosts = [...card.querySelectorAll(hostSelector)].filter(
      (host) => host.getAttribute("data-terminal-session-id") === sid,
    );
    if (matchingHosts.length !== 1) return false;
    const presentationId = matchingHosts[0].getAttribute("data-terminal-presentation-id");
    if (!presentationId) return false;
    const presentationIds = window.__wardianTerminalDebug?.presentationIds?.() ?? [];
    return presentationIds.includes(presentationId) ? presentationId : false;
  }, sessionId, TERMINAL_HOST_SELECTOR), timeoutMs,
  presentationResolutionTimeoutMessage("agent", sessionId));
}

/**
 * Resolves one workbench agent-session host. `surfaceId` is required whenever
 * more than one tab presents the same runtime session; ambiguity fails closed.
 */
export async function resolveAgentSessionTerminalPresentationId(
  driver,
  sessionId,
  { surfaceId, timeoutMs = 20_000 } = {},
) {
  return await driver.wait(async () => await driver.executeScript(
    (sid, requestedSurfaceId, hostSelector) => {
      const panels = [...document.querySelectorAll(
        '[data-testid="surface-panel"][data-surface-type="agent-session"]',
      )].filter((panel) => (
        panel.getAttribute("data-resource-key") === sid &&
        (!requestedSurfaceId || panel.getAttribute("data-surface-id") === requestedSurfaceId)
      ));
      const matchingHosts = panels.flatMap((panel) => [...panel.querySelectorAll(hostSelector)])
        .filter((host) => host.getAttribute("data-terminal-session-id") === sid);
      if (matchingHosts.length !== 1) return false;
      const presentationId = matchingHosts[0].getAttribute("data-terminal-presentation-id");
      if (!presentationId) return false;
      const presentationIds = window.__wardianTerminalDebug?.presentationIds?.() ?? [];
      return presentationIds.includes(presentationId) ? presentationId : false;
    },
    sessionId,
    surfaceId ?? null,
    TERMINAL_HOST_SELECTOR,
  ), timeoutMs, presentationResolutionTimeoutMessage("agent-session", sessionId));
}

export async function readTerminalDebugSnapshot(driver, presentationId) {
  return await driver.executeScript((pid) => (
    window.__wardianTerminalDebug?.snapshot(pid) ?? null
  ), presentationId);
}
