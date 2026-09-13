import assert from "node:assert/strict";

export function approvalSettings(env, profile) {
  const provider = env.WARDIAN_E2E_APPROVAL_PROVIDER?.trim();
  const model = env.WARDIAN_E2E_APPROVAL_MODEL?.trim();
  assert.ok(["claude", "codex", "opencode", "antigravity", "pi"].includes(provider), "Explicit maintained provider required");
  assert.ok(model, "Explicit coordinator-verified usable model required");
  assert.equal(profile.provider, provider, "Profile provider mismatch");
  assert.equal(profile.provider_version, env.WARDIAN_E2E_APPROVAL_PROVIDER_VERSION, "Profile version mismatch");
  for (const field of ["provider_version", "evidence", "ready_text", "prompt_text", "deny_choice", "allow_choice"]) {
    assert.ok(typeof profile[field] === "string" && profile[field].trim().length >= 3, `Observed profile ${field} required`);
  }
  // Profiles encode navigation/select only. Never inject a command or a
  // free-form answer from a profile into a live provider session.
  for (const field of ["deny_keys", "allow_keys"]) {
    assert.match(profile[field] ?? "", /^(?:(?:\x1b\[[AB])|[1-9yn]|\r){1,12}$/, `Invalid ${field}`);
    assert.ok(profile[field].endsWith("\r"), `${field} must select an observed choice`);
  }
  const configs = {
    claude: { type: "claude", permission_mode: "manual" },
    // Installed 0.153.4 rejects untrusted. This parser-supported configuration
    // is a discovery candidate, not proof that a native approval prompt occurs.
    codex: { type: "codex", approval_policy: "on-request", sandbox_mode: "read-only", reasoning_effort: "low" },
    antigravity: { type: "antigravity", dangerously_skip_permissions: false },
  };
  return { provider, model, providerConfig: configs[provider] ?? null };
}

/** Bind the retained profile to the app-resolved CLI/catalog before any prompt.
 * version is the catalog's complete first nonempty --version line, not a guessed semver.
 */
export function assertApprovalCatalog(config, profile, catalog) {
  assert.equal(catalog?.provider, config.provider, "Approval catalog provider mismatch");
  assert.equal(catalog.refresh_error, null, "Approval catalog refresh failed");
  assert.ok(typeof catalog.version === "string" && catalog.version.trim(), "Actual provider version unavailable");
  assert.equal(catalog.version, profile.provider_version, "Observed approval profile does not match actual provider version");
  const model = catalog.models?.find((row) => row.id === config.model);
  assert.ok(model, "Selected approval model absent from actual catalog");
  if (config.provider === "codex") {
    assert.ok(Array.isArray(model.effort_options) && model.effort_options.includes("low"), "Selected approval model must support low effort");
  }
  return { provider_version: catalog.version, selected_model: model.id };
}

export function nativeToolCalls(events, provider, sessionId, filename) {
  return events.filter((row) => row.session_id === sessionId && row.provider === provider &&
    row.metadata?.provider_log === true && row.source && row.kind === "tool_call" && row.role !== "user" &&
    JSON.stringify(row.metadata?.tool_input ?? row.metadata?.input ?? row.command ?? "").includes(filename));
}

/** A current terminal choice plus a native tool call; generic action-needed is insufficient. */
export function assertPendingApproval({ snapshot, status, events, provider, sessionId, filename, marker, profile, beforeIds, fileExists }) {
  assert.equal(fileExists, false, "Stimulated write executed before approval");
  assert.ok(["action needed", "action required"].includes(status?.toLowerCase()), "Provider must require action");
  const grid = snapshot.visible_grid ?? "";
  for (const text of [profile.prompt_text, profile.deny_choice, profile.allow_choice, filename]) {
    assert.ok(grid.includes(text), "Current terminal must show exact operation and both observed choices");
  }
  const calls = nativeToolCalls(events, provider, sessionId, filename).filter((row) => !beforeIds.has(row.id));
  assert.equal(calls.length, 1, "Exactly one new native call must bind the approval operation");
  assert.ok(typeof marker === "string" && marker.length > 0 &&
    JSON.stringify(calls[0].metadata?.tool_input ?? calls[0].metadata?.input ?? calls[0].command ?? "").includes(marker),
  "Native call must contain the requested write content, not merely mention the file");
  return calls[0];
}

function callId(row) {
  return row.metadata?.tool_call_id ?? row.metadata?.call_id ?? row.metadata?.tool_use_id ?? row.turn_id;
}

/** Completion must be linked to the pending native call, never inferred from prose/idle alone. */
export function assertApprovalOutcome({ call, events, provider, sessionId, decision, fileContent, marker, status, snapshot, profile, beforeIds }) {
  assert.equal(status?.toLowerCase(), "idle", "Provider has not settled after decision");
  const grid = snapshot.visible_grid ?? "";
  assert.equal(grid.includes(profile.prompt_text) && grid.includes(profile.deny_choice) && grid.includes(profile.allow_choice),
    false, "Approval choices remain visible");
  const id = callId(call);
  assert.ok(typeof id === "string" && id.length, "Native call correlation unavailable");
  const results = events.filter((row) => row.session_id === sessionId && row.provider === provider &&
    row.metadata?.provider_log === true && row.source && row.kind === "tool_result" && row.role === "tool" &&
    !beforeIds.has(row.id) && callId(row) === id);
  assert.ok(results.length, "No new native result links to the pending request");
  if (decision === "deny") {
    assert.equal(fileContent, null, "Rejected operation created a file");
    assert.ok(results.some((row) => ["failed", "cancelled"].includes(row.status) &&
      /denied|rejected|declined|not approved|permission.*refused/i.test(row.text ?? "")), "No explicit native denial result");
    assert.equal(results.some((row) => row.status === "succeeded"), false, "Rejected operation also succeeded");
  } else {
    assert.equal(decision, "allow", "Unknown approval decision");
    assert.equal(fileContent, marker, "Approved write did not produce exact sentinel content");
    assert.ok(results.some((row) => row.status === "succeeded"), "No native successful completion");
  }
  return { linked_results: results.length, decision, settled: true };
}
