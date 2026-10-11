// @tier ci - Validates CLI team creation through real native IPC with an isolated mock provider.
import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { By } from "selenium-webdriver";
import {
  createNativeHarness,
  ensureNativeAppBuilt,
  prebuiltCliForRun,
  prepareIsolatedHome,
  startNativeSession,
  waitForAppShell,
} from "../lib/harness.mjs";

function runCli(cli, harness, args) {
  const env = { ...process.env, WARDIAN_HOME: harness.isolatedHome };
  delete env.WARDIAN_SESSION_ID;
  const result = spawnSync(cli, args, {
    cwd: harness.repoRoot,
    env,
    encoding: "utf8",
    timeout: 30_000,
    windowsHide: true,
  });
  assert.ifError(result.error);
  return result;
}

function cliJson(cli, harness, args) {
  const result = runCli(cli, harness, args);
  assert.equal(result.status, 0, result.stderr);
  return JSON.parse(result.stdout);
}

test("CLI spawn joins an existing team through the native creation path", { timeout: 180_000 }, async (t) => {
  const harness = await createNativeHarness();
  const skipBuild = process.env.WARDIAN_NATIVE_SKIP_BUILD === "1";
  if (!skipBuild) ensureNativeAppBuilt(harness);
  prepareIsolatedHome(harness);
  // The app's frozen packaged CLI is authoritative; do not replace it with a
  // separately built binary during the run.
  const cli = prebuiltCliForRun(harness, true);
  const session = await startNativeSession(harness);
  t.after(() => session.close());
  await waitForAppShell(session.driver, 20_000);

  const baseArgs = ["agent", "spawn", "--provider", "mock", "--class", "TestClass",
    "--workspace", path.join(harness.repoRoot, "e2e-native")];
  const seed = cliJson(cli, harness, [...baseArgs, "--name", "TeamSeed"]);
  assert.equal(Object.hasOwn(seed, "team"), false);
  const team = cliJson(cli, harness, ["team", "create", "Native Dev", "--agent", seed.agent.uuid]).team;
  const watchlist = cliJson(cli, harness, ["watchlist", "create", "Native Watch"]).watchlist;
  cliJson(cli, harness, ["watchlist", "add-team", watchlist.id, team.id]);
  const watchlistTab = await session.driver.wait(async () => {
    for (const button of await session.driver.findElements(By.css('[data-testid="agent-watchlist"] button[title="Native Watch"]'))) {
      if (await button.isDisplayed()) return button;
    }
    return false;
  }, 20_000, "CLI-created watchlist must appear in the native roster");
  await watchlistTab.click();

  const before = cliJson(cli, harness, ["agent", "list", "--scope", "all"]).agents;
  const invalid = runCli(cli, harness, [...baseArgs, "--name", "RejectedSpawn", "--team", "missing-team"]);
  assert.notEqual(invalid.status, 0);
  assert.equal(JSON.parse(invalid.stderr).error.code, "not_found");
  const after = cliJson(cli, harness, ["agent", "list", "--scope", "all"]).agents;
  assert.deepEqual(after.map(agent => agent.uuid).sort(), before.map(agent => agent.uuid).sort(),
    "invalid team selection must not create an orphan in the live roster");

  const named = cliJson(cli, harness, [...baseArgs, "--name", "NamedTeamMember", "--team", team.name]);
  const identified = cliJson(cli, harness, [...baseArgs, "--name", "IDTeamMember", "--team", team.id]);
  assert.equal(named.team.id, team.id);
  assert.equal(identified.team.id, team.id);
  assert.deepEqual(identified.team.agent_ids, [seed.agent.uuid, named.agent.uuid, identified.agent.uuid]);
  assert.deepEqual(cliJson(cli, harness, ["team", "show", team.id]).team, identified.team);
  const watched = cliJson(cli, harness, ["watchlist", "show", watchlist.id]).watchlist;
  assert.deepEqual(watched.entries, [{ type: "team", team_id: team.id }]);
  const nativeWatchlists = await session.driver.executeAsyncScript((done) => {
    window.__TAURI_INTERNALS__.invoke("load_watchlists").then(
      value => done({ ok: true, value }),
      error => done({ ok: false, error: String(error) }),
    );
  });
  assert.equal(nativeWatchlists.ok, true, nativeWatchlists.error);
  assert.deepEqual(nativeWatchlists.value.teams.find(candidate => candidate.id === team.id).agentIds,
    identified.team.agent_ids, "the native watchlist reader must see the new members");
  assert.deepEqual(nativeWatchlists.value.watchlists.find(candidate => candidate.id === watchlist.id).entries,
    [{ type: "team", teamId: team.id }]);
  const edges = cliJson(cli, harness, ["graph", "show"]).edges;
  for (const peer of [seed.agent.uuid, named.agent.uuid]) {
    const pair = [peer, identified.agent.uuid].sort();
    assert.ok(edges.some(edge => edge.a === pair[0] && edge.b === pair[1]),
      "new membership must seed the normal communication edge");
  }

  const unteamed = cliJson(cli, harness, [...baseArgs, "--name", "DeliberatelyUnteamed"]);
  assert.equal(Object.hasOwn(unteamed, "team"), false);
  const teams = cliJson(cli, harness, ["team", "list"]).teams;
  assert.equal(teams.some(candidate => candidate.agent_ids.includes(unteamed.agent.uuid)), false);
  const roster = cliJson(cli, harness, ["agent", "list", "--scope", "all"]).agents;
  for (const created of [seed, named, identified, unteamed]) {
    assert.ok(roster.some(agent => agent.uuid === created.agent.uuid), "receipt must match the native roster");
  }
  // Observe the selected team watchlist in the actual renderer: notification
  // must reload its members, while an unteamed creation stays outside it.
  await session.driver.wait(() => session.driver.executeScript((teamId) => {
    const tab = document.querySelector('[data-testid="agent-watchlist"] button[title="Native Watch"]');
    const group = document.querySelector('[data-testid="team-block-' + teamId + '"]');
    const visible = element => Boolean(element && element.getBoundingClientRect().width > 0
      && element.getBoundingClientRect().height > 0);
    return tab?.classList.contains("active") && visible(group)
      && ["NamedTeamMember", "IDTeamMember"].every(name => visible(group.querySelector('[aria-label="Agent ' + name + '"]')))
      && !document.querySelector('[data-testid="agent-watchlist"] [aria-label="Agent DeliberatelyUnteamed"]');
  }, team.id), 20_000, "new members must render in the selected team watchlist without a manual refresh");
});
