import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { assertChatHandoffSample, driveChatPreviewProbe } from "./chatPreviewTiming.mjs";

export const MINIMAL_CHAT_TIMING_BYTES = 64 * 1024;

/** Three scheduled cold/warm pairs per surface. No failed opening is resampled. */
export function minimalChatTimingSchedule(homes) {
  return [1, 2, 3].flatMap((repetition) => ["desktop", "remote"].map((surface) => {
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
  catch (error) { record.m1_error = compactError(error); failures.push(error); }
  try { await observeM2(); }
  catch (error) { record.m2_error = compactError(error); failures.push(error); }
  let current;
  try { current = await snapshot(); assertChatHandoffSample(current, options); }
  catch (error) { record.m2_error ??= compactError(error); failures.push(error); }
  if (failures.length === 1) throw failures[0];
  if (failures.length > 1) throw new AggregateError(failures, "Minimal opening and handoff failures", { cause: failures[0] });
  return current;
}

const text = (value, limit = 256) => typeof value === "string" ? value.slice(0, limit) : null;
const finite = (value) => Number.isFinite(value) ? value : null;
const compactError = (error) => ({ code: text(error?.code, 32) ?? "error", message: text(error?.message ?? String(error), 128) });
const reason = (error) => { const value = compactError(error); return `${value.code}: ${value.message}`; };

/** Project only real first-frame evidence and maintained continuity; never serialize DTOs. */
export function compactMinimalChatTiming(report) {
  const cases = [1, 2, 3].flatMap((repetition) => ["desktop", "remote"].flatMap((surface) =>
    ["cold-process-first-chat", "same-process-warm-reopen"].map((temperature) => {
      const label = minimalChatTimingLabel(surface, repetition, temperature);
      const record = report.cases.find((entry) => entry.label === label);
      const phase = record?.observation?.phase;
      const first = phase?.minimal_first_usable;
      const real = first && Number.isFinite(first.at) && Number.isFinite(first.click_at) && first.at >= first.click_at
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
        failure_reasons: [...new Set([record?.error, record?.m1_error, record?.m2_error,
          record?.observation_error, record?.report_error].filter(Boolean).map(reason))] };
    })));
  const failures = report.error ? [reason(report.error)] : [];
  for (const entry of report.surfaces.slice(0, 6)) {
    const prefix = `r${entry.repetition}-${entry.surface}`;
    failures.push(...entry.errors.slice(0, 4).map((error) => `${prefix}: ${reason(error)}`));
    if (entry.errors.length > 4) failures.push(`${prefix}: ${entry.errors.length - 4} additional errors omitted`);
    if (Object.values(entry.cleanup).includes("UNPROVEN")) failures.push(`${prefix}: cleanup unproven`);
  }
  return { status: text(report.status, 64), cases, failure_reasons: failures };
}

/** Separate bounded writer; the established correctness report writer is unchanged. */
export function writeMinimalChatTimingReport(file, report) {
  const bytes = JSON.stringify(compactMinimalChatTiming(report)) + "\n";
  assert.ok(Buffer.byteLength(bytes) <= MINIMAL_CHAT_TIMING_BYTES, "Minimal timing report exceeds its fixed byte cap");
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, bytes);
}
