// @tier nightly — App-free compact timing mode contracts; never starts an app or provider.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { chatPreviewHomePaths, recordChatTimingCase } from "../lib/chatPreviewTiming.mjs";
import { MINIMAL_CHAT_TIMING_BYTES, minimalChatTimingSchedule, minimalChatTimingLabel, driveMinimalChatProbe,
  observeMinimalChatOpening, compactMinimalChatTiming, writeMinimalChatTimingReport, remoteReopenLabels } from "../lib/chatPreviewMinimalTiming.mjs";

const temperatures = ["cold-process-first-chat", "same-process-warm-reopen"];
const label = minimalChatTimingLabel("desktop", 1, temperatures[0]);
function observedReport() {
  return { status: "FAILED", cases: [{ label, status: "FAILED", error: { code: "handoff_failure", message: "Retain this failure" },
    observation: { phase: { clickAt: 100, usableAt: 1_335.125,
      minimal_first_usable: { click_at: 100, at: 1_335.125, frames: [1_319, 1_335.125], anchor_top: 12.25, receipt_sequence: 3 },
      minimal_anchor: { verified: true, frames: 300, delta_px: 3.5, top: 15.75, at: 2_000 },
      continuity: { source_switches: 1, last: { at: 30_100, anchor_top: 15.75, content_count: 1 } } } } }], surfaces: [] };
}

test("three repetitions schedule both surfaces with fresh fixed-width homes and exactly twelve opening slots", () => {
  const runId = "0123456789abcdef";
  const homes = chatPreviewHomePaths({ runId,
    baseHome: path.join(path.parse(process.cwd()).root, "wpv8", `wardian-e2e-native-${runId}`) });
  const schedule = minimalChatTimingSchedule(homes);
  assert.deepEqual(schedule.map(({ repetition, surface }) => [repetition, surface]), [
    [1, "desktop"], [1, "remote"], [2, "desktop"], [2, "remote"], [3, "desktop"], [3, "remote"],
  ]);
  assert.equal(new Set(schedule.map((entry) => entry.home)).size, 6);
  for (const entry of schedule) {
    assert.equal(entry.mode, "pending");
    assert.equal(entry.home.length, homes.cases.find((value) => value.surface === entry.surface && value.mode === "pending").home.length);
  }
  const report = compactMinimalChatTiming({ cases: [], surfaces: [] });
  assert.deepEqual(report.cases.map((entry) => entry.label), schedule.flatMap((entry) =>
    temperatures.map((temperature) => minimalChatTimingLabel(entry.surface, entry.repetition, temperature))));
  assert.ok(report.cases.every((entry) => entry.status === "NOT_ATTEMPTED" && entry.first_usable_ms === null && entry.anchor_delta_px === null));
});

test("Remote repeat diagnostics schedule seven openings for one home and distinguish renderer changes from proven switches", () => {
  const homes = { cases: [{ surface: "remote", mode: "pending", home: "synthetic-r-p" }] };
  assert.deepEqual(minimalChatTimingSchedule(homes, { remoteRepeatedOpen: true }), [
    { surface: "remote", mode: "pending", repetition: 1, home: "synthetic-r-1" },
  ]);
  const report = { ...observedReport(), remoteRepeatedOpen: true };
  const record = report.cases[0];
  record.label = remoteReopenLabels[0];
  const phase = record.observation.phase;
  phase.continuity = { source_switches: 0, samples: [
    { renderer: "legacy", representation: { kind: "legacy" } },
    { renderer: "shared", representation: { kind: "unknown" } },
    { renderer: "shared", representation: { kind: "provisional" } },
  ] };
  const compact = compactMinimalChatTiming(report);
  assert.equal(compact.cases.length, 7);
  assert.equal(new Set(compact.cases.map((entry) => entry.label)).size, 7);
  assert.equal(compact.cases[0].source_switch_count, 0);
  assert.equal(compact.cases[0].first_usable_ms, null);
  assert.deepEqual(compact.cases[0].evidence.renderer_path, ["legacy/legacy", "shared/unknown", "shared/provisional"]);
  assert.ok(compact.cases.slice(1).every((entry) => entry.status === "NOT_ATTEMPTED"));
  record.m1_observed = { usable_ms: 1_235.125 };
  assert.equal(compactMinimalChatTiming(report).cases[0].first_usable_ms, 1_235.125);
});

test("compact writer retains an over-budget first usable and real anchor delta despite handoff failure and oversized raw details", () => {
  const report = observedReport();
  report.cases[0].observation.unneeded_raw_dto = "unserialized-raw-body".repeat(64 * 1024);
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "wardian-minimal-timing-"));
  try {
    const file = path.join(directory, "result.json"); writeMinimalChatTimingReport(file, report);
    assert.ok(fs.statSync(file).size < MINIMAL_CHAT_TIMING_BYTES);
    const saved = JSON.parse(fs.readFileSync(file, "utf8"));
    assert.equal(saved.cases.length, 12);
    assert.deepEqual(Object.keys(saved).sort(), ["cases", "failure_reasons", "status"]);
    assert.deepEqual(Object.keys(saved.cases[0]).sort(), ["anchor_delta_px", "failure_reasons", "first_usable_ms", "label", "source_switch_count", "status"]);
    assert.equal(saved.cases[0].first_usable_ms, 1_235.125);
    assert.equal(saved.cases[0].source_switch_count, 1); assert.equal(saved.cases[0].anchor_delta_px, 3.5);
    assert.match(saved.cases[0].failure_reasons[0], /^handoff_failure:/); assert.equal(saved.cases[0].status, "FAILED");
    assert.equal(saved.cases[2].status, "NOT_ATTEMPTED");
    assert.ok(!fs.readFileSync(file, "utf8").includes("unserialized-raw-body"));
  } finally { fs.rmSync(directory, { recursive: true, force: true }); }
});

test("missing/mismatched first-frame evidence or missing end anchor stays unmeasured instead of substituting later DOM", () => {
  for (const mutate of [
    (p) => { delete p.minimal_first_usable; },
    (p) => { p.minimal_first_usable.at++; },
    (p) => { p.minimal_first_usable.click_at++; },
    (p) => { p.minimal_first_usable.frames[0] = 0; },
    (p) => { p.minimal_first_usable.at = p.usableAt = undefined; },
  ]) {
    const report = observedReport(); mutate(report.cases[0].observation.phase);
    const result = compactMinimalChatTiming(report).cases[0];
    assert.equal(result.first_usable_ms, null); assert.equal(result.anchor_delta_px, null);
  }
  const report = observedReport(); report.cases[0].observation.phase.minimal_anchor.verified = false;
  const result = compactMinimalChatTiming(report).cases[0];
  assert.equal(result.first_usable_ms, 1_235.125); assert.equal(result.anchor_delta_px, null);
});

test("all twelve failed openings and six surface errors remain within the compact cap even with maximal multibyte error strings", () => {
  const long = "測".repeat(10_000); const error = { code: long, message: long };
  const report = { status: long, artifact: { source_commit: long, app_sha256: long, cli_sha256: long }, cases: [], surfaces: [] };
  for (let repetition = 1; repetition <= 3; repetition++) for (const surface of ["desktop", "remote"]) {
    report.surfaces.push({ repetition, surface, status: long, errors: Array(100).fill(error), cleanup: { observer: long, browser: long, session: long, tls: long } });
    for (const temperature of temperatures) report.cases.push({ ...observedReport().cases[0],
      label: minimalChatTimingLabel(surface, repetition, temperature), status: long, error, m1_error: error, m2_error: error,
      observation_error: long, report_error: long });
  }
  const result = compactMinimalChatTiming(report);
  assert.equal(result.cases.length, 12);
  assert.equal(result.failure_reasons.filter((entry) => entry.includes("96 additional errors omitted")).length, 6);
  assert.ok(Buffer.byteLength(JSON.stringify(result)) < MINIMAL_CHAT_TIMING_BYTES);
});

test("minimal driver dispatches one genuine input and releases handles, rejecting retries/replaced subjects without renewing its deadline", async () => {
  let clock = 100; const remaining = []; const released = []; let inputs = 0; let index = 0;
  await driveMinimalChatProbe({ now: () => clock,
    next: async (ms) => { remaining.push(ms); return index++ === 0 ? { kind: "input", attempt: 1 } : { kind: "complete" }; },
    isCurrent: async () => true, act: async () => { inputs++; clock += 100; }, release: async (plan) => released.push(plan.kind) });
  assert.equal(inputs, 1); assert.deepEqual(released, ["input", "complete"]);
  assert.equal(remaining[0] - remaining[1], 100);
  for (const [attempt, current] of [[2, true], [1, false]]) {
    let released = false;
    await assert.rejects(driveMinimalChatProbe({ next: async () => ({ kind: "input", attempt }),
      isCurrent: async () => current, act: async () => { inputs++; }, release: async () => { released = true; } }),
    /Minimal timing/);
    assert.equal(released, true);
  }
  assert.equal(inputs, 1);
});

test("M1 and handoff failures remain recorded while the scheduled Desktop/Remote openings continue without resampling", async () => {
  const report = { cases: [], surfaces: [] }; const attempted = [];
  const original = Object.assign(new Error("original input failure"), { code: "chat_input_timeout" });
  const handoff = Object.assign(new Error("actual handoff failure"), { code: "missing_qualified_handoff" });
  for (let repetition = 1; repetition <= 3; repetition++) for (const surface of ["desktop", "remote"]) for (const temperature of temperatures) {
    const name = minimalChatTimingLabel(surface, repetition, temperature);
    await assert.rejects(recordChatTimingCase(report, name, () => observeMinimalChatOpening({ record: report.cases.at(-1),
      measureM1: async () => { attempted.push(name); throw original; },
      observeM2: async () => { throw handoff; }, snapshot: async () => ({}), options: { sourceRole: "candidate" },
    }), async () => ({}), async () => { assert.ok(Buffer.byteLength(JSON.stringify(compactMinimalChatTiming(report))) < MINIMAL_CHAT_TIMING_BYTES); }),
    (error) => error.cause === original && error.errors.includes(handoff));
  }
  assert.equal(attempted.length, 12); assert.equal(new Set(attempted).size, 12);
  assert.ok(report.cases.every((entry) => entry.status === "FAILED" && entry.m1_error.code === original.code && entry.m2_error.code === handoff.code));
  assert.ok(compactMinimalChatTiming(report).cases.every((entry) => entry.first_usable_ms === null && entry.failure_reasons.some((value) => value.startsWith(handoff.code))));
});
