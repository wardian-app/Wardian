import assert from "node:assert/strict";

/** Select only a live, non-default model that explicitly advertises low effort. */
export function selectCodexLowModel(models) {
  return Array.isArray(models) ? models.find((model) =>
    typeof model?.id === "string" && model.id.trim() && model.is_default === false &&
    Array.isArray(model.effort_options) && model.effort_options.includes("low")) ?? null : null;
}

/** Temporary automation assignments have no registered conversation to inherit. */
export function temporaryProviderAssignment({ provider, workspace, model }) {
  return { target_type: "temporary_provider", provider, workspace, model,
    ...(provider === "codex" ? { effort: "low" } : {}) };
}

/** Accept normalized answer fields only; arbitrary provider event packets are not answers. */
export function temporaryProviderOutputText(output) {
  if (typeof output === "string") return output;
  assert.ok(output && typeof output === "object" && !Array.isArray(output), "Unsupported temporary provider output shape");
  const fields = ["text", "response"].filter((key) => Object.hasOwn(output, key));
  assert.ok(fields.length > 0 && fields.every((key) => typeof output[key] === "string"),
    "Unsupported temporary provider output shape");
  const values = fields.map((key) => output[key]);
  assert.ok(values.every((value) => value === values[0]), "Ambiguous temporary provider output fields");
  return values[0];
}

/** An echoed marker in a raw event stream cannot satisfy this answer-only prompt. */
export function assertTemporaryProviderAnswer(output, marker) {
  assert.equal(temporaryProviderOutputText(output).trim(), marker,
    "Temporary provider must return only the requested marker");
}
