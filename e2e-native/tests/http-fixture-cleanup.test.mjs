// @tier ci — Real HTTP connection ownership and after-hook cleanup; no app or provider.
import test from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import net from "node:net";
import { once } from "node:events";

import { closeHttpFixture } from "../lib/httpFixture.mjs";

async function connectedFixture() {
  const server = http.createServer((_request, response) => response.end("fixture"));
  const accepted = Promise.withResolvers();
  server.once("connection", (socket) => accepted.resolve(socket));
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const client = net.createConnection({ host: "127.0.0.1", port: server.address().port });
  client.on("error", () => {});
  await once(client, "connect");
  const socket = await accepted.promise;
  return {
    server,
    client,
    socket,
    socketClosed: once(socket, "close"),
    clientClosed: once(client, "close"),
  };
}

async function releaseFixture(fixture) {
  fixture.client.destroy();
  fixture.server.closeAllConnections();
  if (fixture.server.listening) {
    await new Promise((resolve, reject) => {
      fixture.server.close((error) => (error ? reject(error) : resolve()));
    });
  }
  await Promise.all([fixture.socketClosed, fixture.clientClosed]);
}

test("HTTP fixture cleanup releases its live connection before a later client shutdown hook", { timeout: 5000 }, async (t) => {
  const fixture = await connectedFixture();
  const foreign = await connectedFixture();
  const closeIssued = Promise.withResolvers();
  const trace = [];
  let child;
  t.signal.addEventListener("abort", () => {
    fixture.client.destroy();
    foreign.client.destroy();
    fixture.server.closeAllConnections();
    foreign.server.closeAllConnections();
  }, { once: true });
  try {
    child = t.test("fixture closes before client cleanup", async (childTest) => {
      childTest.after(async () => {
        trace.push("server-close-enter");
        const closed = closeHttpFixture(fixture.server);
        closeIssued.resolve();
        await closed;
        trace.push("server-close-end");
      });
      childTest.after(() => {
        trace.push("client-close-enter");
        fixture.client.destroy();
      });
    });
    await closeIssued.promise;
    assert.equal(
      fixture.socket.destroyed,
      true,
      "the fixture must dispose its accepted live connection without waiting for the later client hook",
    );
    await child;
    await Promise.all([fixture.socketClosed, fixture.clientClosed]);
    assert.deepEqual(trace, ["server-close-enter", "server-close-end", "client-close-enter"]);
    assert.equal(fixture.server.listening, false);
    assert.equal(foreign.server.listening, true);
    assert.equal(foreign.socket.destroyed, false, "another fixture's connection must remain live");
  } finally {
    // Rescue only these owned endpoints on a failed assertion, so the red case
    // demonstrates the dependency without waiting for a socket timeout.
    await releaseFixture(fixture);
    await releaseFixture(foreign);
    await child;
  }
});

test("HTTP fixture cleanup completes after a normal keep-alive response", async () => {
  const server = http.createServer((_request, response) => response.end("fixture"));
  const agent = new http.Agent({ keepAlive: true });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  try {
    const body = await new Promise((resolve, reject) => {
      http.get({ host: "127.0.0.1", port: server.address().port, agent }, (response) => {
        const chunks = [];
        response.on("data", (chunk) => chunks.push(chunk));
        response.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
        response.on("error", reject);
      }).on("error", reject);
    });
    assert.equal(body, "fixture");
    await closeHttpFixture(server);
    assert.equal(server.listening, false);
  } finally {
    agent.destroy();
    server.closeAllConnections();
    if (server.listening) {
      await new Promise((resolve, reject) => {
        server.close((error) => (error ? reject(error) : resolve()));
      });
    }
  }
});
