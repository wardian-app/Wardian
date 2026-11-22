import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { CHAT_PREVIEW_RULER_VERSION, assertChatHandoffSample, driveChatPreviewProbe } from "./chatPreviewTiming.mjs";

export const MINIMAL_CHAT_TIMING_BYTES = 64 * 1024;
const openingTemperatures = ["cold-process-first-chat", "same-process-warm-reopen"];
export const remoteReopenLabels = Object.freeze([
  ...openingTemperatures.map((temperature) => minimalChatTimingLabel("remote", 1, temperature)),
  ...Array.from({ length: 5 }, (_, index) => `r1-remote-pending-consecutive-reopen-${index + 1}`),
]);

/** Use distinct homes for independent samples and one home for the Remote reopen sequence. */
export function minimalChatTimingSchedule(homes, { remoteRepeatedOpen = false } = {}) {
  const repetitions = remoteRepeatedOpen ? [1] : [1, 2, 3];
  const surfaces = remoteRepeatedOpen ? ["remote"] : ["desktop", "remote"];
  return repetitions.flatMap((repetition) => surfaces.map((surface) => {
    const original = homes.cases.find((entry) => entry.surface === surface && entry.mode === "pending");
    assert.ok(original?.home.endsWith(`-${surface[0]}-p`), "Expected maintained pending home layout");
    // Replace the final fixed-width mode character; fixture path/byte widths stay unchanged.
    return { surface, mode: "pending", repetition, home: original.home.slice(0, -1) + repetition };
  }));
}

export function minimalChatTimingLabel(surface, repetition, temperature) {
  return `r${repetition}-${surface}-pending-${temperature}`;
}

/** Reuse real input/deadline ownership, but refuse a second or replaced probe. */
export async function driveMinimalChatProbe(adapter) {
  let dispatched = false;
  return driveChatPreviewProbe({ ...adapter,
    isCurrent: async (plan) => {
      // Check inside the maintained driver's try/finally so rejected handles are still released.
      assert.equal(plan.attempt, 1, "Minimal timing does not retry a probe");
      assert.equal(await adapter.isCurrent(plan), true, "Minimal timing probe subject changed before input");
      return true;
    },
    act: async (plan) => {
      assert.equal(dispatched, false, "Minimal timing does not repeat input");
      dispatched = true;
      return adapter.act(plan);
    },
  });
}

/** Always finish the existing passive window, retaining M1 and handoff failures. */
export async function observeMinimalChatOpening({ measureM1, observeM2, snapshot, record, options }) {
  const failures = [];
  try { await measureM1(); }
  catch (error) {
    record.m1_error = compactError(error);
    failures.push(error);
    if (options.remoteRepeatedOpen) {
      try { record.failure_evidence = remoteOpeningEvidence({ ...record, observation: await snapshot() }); }
      catch (snapshotError) { failures.push(snapshotError); }
    }
  }
  try { await observeM2(); }
  catch (error) { record.m2_error = compactError(error); failures.push(error); }
  let current;
  try { current = await snapshot(); assertChatHandoffSample(current, options); }
  catch (error) { record.m2_error ??= compactError(error); failures.push(error); }
  if (failures.length === 1) throw failures[0];
  if (failures.length > 1) throw new AggregateError(failures, "Minimal opening and handoff failures", { cause: failures[0] });
  return current;
}

/** Preserve the first usable verdict at its own endpoint while still checking late health. */
export function remoteReopenSample(record, final, options) {
  assert.ok(record.m1_observed, "Missing verified first-usable witness");
  return { ruler_version: CHAT_PREVIEW_RULER_VERSION, m1: record.m1_observed, m2: assertChatHandoffSample(final, options) };
}

const text = (value, limit = 256) => typeof value === "string" ? value.slice(0, limit) : null;
const finite = (value) => Number.isFinite(value) ? value : null;
const compactError = (error) => ({ code: text(error?.code, 32) ?? "error", message: text(error?.message ?? String(error), 128) });
const reason = (error) => { const value = compactError(error); return `${value.code}: ${value.message}`; };

function remoteOpeningEvidence(record) {
  const observation = record?.observation;
  const phase = observation?.phase;
  const phaseIndex = observation?.phases?.length - 1;
  const receipts = observation?.receipts?.filter((entry) => entry.last_request?.phase_index === phaseIndex) ?? [];
  return {
    capture_before_click: record?.capture?.before?.status ?? null,
    capture_after_click: record?.capture?.after?.status ?? null,
    pending_at_click_proved: record?.capture?.proof?.status === "PENDING_BRACKETED_TRUSTED_CLICK",
    selector: phase?.selector_diagnostics?.last ?? null,
    readable_proved: Number.isFinite(phase?.readableAt),
    usable_proved: record?.m1_observed !== undefined,
    opening_representation: phase?.continuity?.opening_representation ?? null,
    renderer_path: (phase?.continuity?.samples ?? [])
      .map(({ renderer, representation }) => `${renderer}/${representation?.kind ?? "unproven"}`)
      .filter((renderer, index, renderers) => index === 0 || renderer !== renderers[index - 1]).slice(0, 16),
    projection_witness_ms: Number.isFinite(phase?.projectionAt) ? phase.projectionAt - phase.clickAt : null,
    current_native: observation?.health?.current_native ?? null,
    current_projection: observation?.health?.current_projection ?? null,
    transcript_error_visible: observation?.diagnostics?.transcript_error_visible ?? null,
    distinct_receipts: receipts.length,
    native_receipts: receipts.filter((entry) => entry.nativeOpening?.verified).length,
    projected_receipts: receipts.filter((entry) => entry.presentation?.turns > 0).length,
    target_projected_receipts: receipts.filter((entry) => entry.presentation?.target_turns?.length > 0).length,
    unscoped_resets: receipts.filter((entry) => entry.control === "unscoped_reset").length,
    latest_progress: text(receipts.at(-1)?.progress, 32),
    faults: (observation?.faults ?? []).slice(0, 4).map((fault) => text(fault, 96)),
  };
}

/** Project only real first-frame evidence and maintained continuity; never serialize DTOs. */
export function compactMinimalChatTiming(report) {
  const labels = report.remoteRepeatedOpen ? remoteReopenLabels : [1, 2, 3].flatMap((repetition) =>
    ["desktop", "remote"].flatMap((surface) => openingTemperatures.map((temperature) =>
      minimalChatTimingLabel(surface, repetition, temperature))));
  const cases = labels.map((label) => {
    const record = report.cases.find((entry) => entry.label === label);
    const phase = record?.observation?.phase;
    const first = phase?.minimal_first_usable;
    const real = first && (!report.remoteRepeatedOpen || record.m1_observed !== undefined)
      && Number.isFinite(first.at) && Number.isFinite(first.click_at) && first.at >= first.click_at
      && first.at === phase.usableAt && first.click_at === phase.clickAt && first.frames?.length === 2
      && first.frames[1] === first.at && Number.isFinite(first.frames[0]) && first.frames[0] >= first.click_at
      && first.frames[0] <= first.at;
    const peak = phase?.minimal_anchor;
    const anchor = real && peak?.verified === true && peak.frames > 1 && Number.isFinite(first.anchor_top)
      && Number.isFinite(peak.delta_px) && Number.isFinite(peak.top) && peak.at >= first.at
      && peak.delta_px === peak.top - first.anchor_top;
    return { label,
      status: text(record?.status, 64) ?? "NOT_ATTEMPTED",
      first_usable_ms: real ? finite(first.at - first.click_at) : null,
      source_switch_count: finite(phase?.continuity?.source_switches),
      anchor_delta_px: anchor ? peak.delta_px : null,
      ...(report.remoteRepeatedOpen ? { evidence: remoteOpeningEvidence(record), failure_evidence: record?.failure_evidence } : {}),
      failure_reasons: [...new Set([record?.error, record?.m1_error, record?.m2_error,
        record?.observation_error, record?.report_error].filter(Boolean).map(reason))] };
  });
  const failures = report.error ? [reason(report.error)] : [];
  for (const entry of report.surfaces.slice(0, 6)) {
    const prefix = `r${entry.repetition}-${entry.surface}`;
    failures.push(...entry.errors.slice(0, 4).map((error) => `${prefix}: ${reason(error)}`));
    if (entry.errors.length > 4) failures.push(`${prefix}: ${entry.errors.length - 4} additional errors omitted`);
    if (Object.values(entry.cleanup).includes("UNPROVEN")) failures.push(`${prefix}: cleanup unproven`);
  }
  return { status: text(report.status, 64),
    ...(report.remoteRepeatedOpen ? { method: "remote-same-session-reopen-v1", artifact_source: report.artifact?.source_commit } : {}),
    cases, failure_reasons: failures };
}

/** Separate bounded writer; the established correctness report writer is unchanged. */
export function writeMinimalChatTimingReport(file, report) {
  const bytes = JSON.stringify(compactMinimalChatTiming(report)) + "\n";
  assert.ok(Buffer.byteLength(bytes) <= MINIMAL_CHAT_TIMING_BYTES, "Minimal timing report exceeds its fixed byte cap");
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, bytes);
}
