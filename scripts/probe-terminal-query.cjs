const fs = require("node:fs");

const outputPath = process.argv[2];
const timeoutMs = Number(process.argv[3] || 800);

if (!outputPath) {
  console.error("usage: node probe-terminal-query.cjs <output-json> [timeout-ms]");
  process.exit(2);
}

const queries = [
  { name: "cursor_position", sequence: "\x1b[6n" },
  { name: "window_pixels", sequence: "\x1b[14t" },
  { name: "text_area_chars", sequence: "\x1b[18t" },
];

const chunks = [];
const startedAt = new Date().toISOString();

function escapeControl(value) {
  return value
    .replace(/\x1b/g, "\\x1b")
    .replace(/\r/g, "\\r")
    .replace(/\n/g, "\\n")
    .replace(/\t/g, "\\t");
}

function parseResponses(response) {
  const parsed = [];
  const windowPattern = /\x1b\[([0-9;]+)t/g;
  for (const match of response.matchAll(windowPattern)) {
    const parts = match[1].split(";").map((part) => Number(part));
    if (parts.length === 3 && parts.every((part) => Number.isFinite(part))) {
      parsed.push({
        raw: escapeControl(match[0]),
        code: parts[0],
        height: parts[1],
        width: parts[2],
      });
    }
  }
  const cursorPattern = /\x1b\[([0-9]+);([0-9]+)R/g;
  for (const match of response.matchAll(cursorPattern)) {
    parsed.push({
      name: "cursor_position",
      raw: escapeControl(match[0]),
      row: Number(match[1]),
      column: Number(match[2]),
    });
  }
  return parsed;
}

function finish() {
  if (process.stdin.isTTY && typeof process.stdin.setRawMode === "function") {
    process.stdin.setRawMode(false);
  }
  const response = Buffer.concat(chunks).toString("utf8");
  fs.writeFileSync(
    outputPath,
    `${JSON.stringify(
      {
        started_at: startedAt,
        timeout_ms: timeoutMs,
        queries: queries.map((query) => ({
          name: query.name,
          sequence: escapeControl(query.sequence),
        })),
        response: escapeControl(response),
        parsed: parseResponses(response),
      },
      null,
      2,
    )}\n`,
    "utf8",
  );
  process.exit(0);
}

if (process.stdin.isTTY && typeof process.stdin.setRawMode === "function") {
  process.stdin.setRawMode(true);
}
process.stdin.resume();
process.stdin.on("data", (chunk) => chunks.push(Buffer.from(chunk)));

process.stdout.write(queries.map((query) => query.sequence).join(""));
setTimeout(finish, timeoutMs);
