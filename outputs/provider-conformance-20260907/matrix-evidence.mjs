import { createHash } from 'node:crypto';
import { codexMessagingObservations } from '../../e2e-native/lib/codex-messaging-conformance-evidence.mjs';

export { CODEX_MESSAGING_FUNCTIONS } from '../../e2e-native/lib/codex-messaging-conformance-evidence.mjs';

const severity = { fail: 6, blocked: 5, reported: 4, untested: 3, pass: 2, intentional_skip: 1 };

/** Frozen build13 identity, before the later one-line change. Not a current-build verdict. */
export const CODEX_BUILD13 = Object.freeze({
  provider: 'codex', model: 'gpt-5.6-luna', effort: 'low', providerVersion: '0.154.0-alpha.6',
  sourceCommit: '06be93d983bbb239b971b3c192158512596557b6',
  manifestSha256: '53a33a2e3d0bb0e71ce1cd659b33a0fb59ef9c29ead945ffa131caa323493305',
  harnessSha256: 'f8f5a5369f41e54e8c77c5a5eaafad6943c87b68d1893ca16ea69d37cf807f32',
});
const BUILD13_REPORTS = new Set([
  '8573107773cc06890df45996e811e1d75447e747cd497c75b7da7abfc0fb5021',
  '23c3a264acfcc36cbbe73435d9eed7d237fc1b4c8197e4dba5c2b87af8fa1761',
]);

/** Pure import of the two retained reports. Caller reads bytes; no ledger/workbook mutation. */
export function importCodexBuild13Messaging({ reportBytes, manifestBytes }) {
  if (!Buffer.isBuffer(reportBytes)) throw new Error('Expected report bytes');
  const reportSha256 = createHash('sha256').update(reportBytes).digest('hex');
  if (!BUILD13_REPORTS.has(reportSha256)) throw new Error('Unrecognized build13 report SHA');
  return codexMessagingObservations({ reportBytes, manifestBytes, expected: { ...CODEX_BUILD13, reportSha256 } });
}

/** Filter before scenario selection: v2 evidence cannot implicitly satisfy legacy cells. */
export function selectMatrixCellEvidence(records, provider, functionId, qualify) {
  return selectFunctionEvidence(records.filter(record => record.provider === provider && record.function === functionId), qualify);
}

// Only these composite functions require multiple named assertions.
const requiredScenarios = {
  chat_log_link: ['chat/current-log-link', 'chat/fresh-session-log-link'],
  user_provenance: ['chat/user-prompt-provenance', 'chat/tool-result-provenance'],
};

function knownScenario(record) {
  if (requiredScenarios[record.function]?.includes(record.case)) return record.case;
  // Inspected baseline: the replayed human event lacked input_origin. This is
  // not evidence about tool-result exclusion (nor Pi's request-root defect).
  if (record.function === 'user_provenance' && record.provider === 'antigravity'
    && record.case === 'mailbox-short' && record.phase === 'after'
    && (record.wardian_revision || record.build) === '39d996c896de2e5a4883ff325984889c851348e2') {
    return 'chat/user-prompt-provenance';
  }
  return undefined;
}

/** Stable importer normalization; preserve explicit groups and original observations. */
export function normalizeFunctionEvidence(record) {
  const scenario = knownScenario(record);
  return record.coverage_group || !scenario ? record : { ...record, coverage_group: scenario };
}

/** Select one provider/function cell; independent scenarios survive partial retests. */
export function selectFunctionEvidence(records, qualify = record => record) {
  if (new Set(records.map(record => record.provider)).size > 1
    || new Set(records.map(record => record.function)).size > 1) {
    throw new Error('Select evidence for one provider and function');
  }
  const required = requiredScenarios[records[0]?.function];
  const groups = new Map();
  for (const original of records) {
    const record = normalizeFunctionEvidence(qualify(original));
    if (!severity[record.status]) throw new Error(`Unknown result status: ${record.status}`);
    // Even an explicit shared group must not collapse distinct known assertions.
    // Unknown cases remain separate obligations, not aliases for named scenarios.
    const group = JSON.stringify([record.coverage_group || 'core',
      required ? knownScenario(record) || record.case || null : null]);
    const previous = groups.get(group);
    const entry = { record, nonAssertion: record.status === 'untested' };
    const dateOrder = String(record.date).localeCompare(String(previous?.record.date));
    // Untested/no-stimulus observations cannot retest a retained failure, in either order.
    const retainedFailure = previous && (
      entry.nonAssertion && severity[previous.record.status] > severity.untested
      || previous.nonAssertion && severity[record.status] > severity.untested);
    const replace = retainedFailure ? previous.nonAssertion
      : !previous || dateOrder > 0 || (dateOrder === 0 && severity[record.status] > severity[previous.record.status]);
    if (replace) groups.set(group, entry);
  }
  const selected = [...groups.values()].map(entry => entry.record);
  const result = selected.sort((left, right) => severity[right.status] - severity[left.status]
    || String(right.date).localeCompare(String(left.date)))[0];
  if (required && result?.status === 'pass') {
    // Legacy aliases can carry failures, but only exact current assertion cases
    // can establish completeness. A function-wide pass is insufficient.
    const missing = required.filter(scenario => !selected.some(record => record.case === scenario && record.status === 'pass'));
    if (missing.length) return { ...result, status: 'untested', failure: undefined,
      evidence: `No qualifying pass for required scenarios: ${missing.join(', ')}.` };
  }
  return result;
}
