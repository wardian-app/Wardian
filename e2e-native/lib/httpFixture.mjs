/**
 * Stop a test-owned HTTP fixture and its accepted HTTP connections, then await close.
 *
 * Chromium can retain a connection without sending an HTTP request. Waiting
 * for server.close alone leaves that socket live and gates a later after hook
 * that would stop the browser. Stop accepting first, then drain only this
 * fixture's connections so teardown does not depend on the browser exiting.
 *
 * @param {import("node:http").Server} server
 * @returns {Promise<void>}
 */
export function closeHttpFixture(server) {
  return new Promise((resolve, reject) => {
    server.close((error) => (error ? reject(error) : resolve()));
    server.closeAllConnections();
  });
}
