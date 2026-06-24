import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import trackerCsv from "../../docs/feature-user-story-status.csv?raw";

const repoRoot = process.cwd();
const expectedHeaders = [
  "id",
  "area",
  "feature",
  "user_story",
  "expected_behavior",
  "code_evidence",
  "docs_evidence",
  "test_layer",
  "feature_status",
  "test_status",
  "next_verification",
  "defects",
  "notes",
] as const;

type TrackerHeader = (typeof expectedHeaders)[number];
type TrackerRow = Record<TrackerHeader, string>;

function parseCsv(text: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let quoted = false;

  for (let index = 0; index < text.length; index += 1) {
    const character = text[index];

    if (quoted) {
      if (character === "\"") {
        if (text[index + 1] === "\"") {
          field += "\"";
          index += 1;
        } else {
          quoted = false;
        }
      } else {
        field += character;
      }
      continue;
    }

    if (character === "\"") {
      quoted = true;
    } else if (character === ",") {
      row.push(field);
      field = "";
    } else if (character === "\n") {
      row.push(field.replace(/\r$/, ""));
      rows.push(row);
      row = [];
      field = "";
    } else {
      field += character;
    }
  }

  if (field.length > 0 || row.length > 0) {
    row.push(field.replace(/\r$/, ""));
    rows.push(row);
  }

  return rows.filter((fields) => fields.length > 1 || fields[0] !== "");
}

function trackerRows(): TrackerRow[] {
  const rows = parseCsv(trackerCsv);
  const [headers, ...records] = rows;
  expect(headers).toEqual(expectedHeaders);

  records.forEach((record, index) => {
    expect(record, `row ${index + 2} should have the canonical column count`).toHaveLength(expectedHeaders.length);
  });

  return records.map((record) =>
    Object.fromEntries(expectedHeaders.map((header, index) => [header, record[index] ?? ""])) as TrackerRow,
  );
}

function evidencePaths(row: TrackerRow): string[] {
  return [row.code_evidence, row.docs_evidence]
    .flatMap((evidence) => evidence.split(";"))
    .map((entry) => entry.trim())
    .filter(Boolean);
}

function pathExistsOrIsDirectoryPrefix(relativePath: string): boolean {
  const normalizedPath = relativePath.replace(/\\/g, "/");
  const absolutePath = join(repoRoot, normalizedPath);
  return existsSync(absolutePath);
}

function markdownFiles(root: string): string[] {
  return readdirSync(root, { withFileTypes: true }).flatMap((entry) => {
    const entryPath = join(root, entry.name);
    if (entry.isDirectory()) return markdownFiles(entryPath);
    return entry.name.endsWith(".md") ? [entryPath.replace(/\\/g, "/")] : [];
  });
}

function documentedMarkdownPaths(rows: TrackerRow[]): Set<string> {
  return new Set(rows.flatMap((row) => row.docs_evidence.split(";").map((entry) => entry.trim())));
}

function isNonStoryReferenceDoc(relativePath: string): boolean {
  return (
    relativePath === "docs/specs/template.md" ||
    relativePath.startsWith("docs/research/") ||
    relativePath.startsWith("docs/assets/")
  );
}

function cargoPackageManifests(): Array<{ name: string; directory: string }> {
  return ["src-tauri/Cargo.toml", "crates/wardian-core/Cargo.toml", "crates/wardian-cli/Cargo.toml"].map(
    (manifestPath) => {
      const manifest = readFileSync(join(repoRoot, manifestPath), "utf8");
      const packageName = manifest.match(/^\[package\][\s\S]*?^name\s*=\s*"([^"]+)"/m)?.[1];
      expect(packageName, `${manifestPath} should declare a package name`).toBeDefined();

      return {
        name: packageName ?? "",
        directory: manifestPath.replace(/\/Cargo\.toml$/, ""),
      };
    },
  );
}

function cargoIntegrationTests(packageDirectory: string): Set<string> {
  const testsDirectory = join(repoRoot, packageDirectory, "tests");
  if (!existsSync(testsDirectory)) return new Set();

  return new Set(
    readdirSync(testsDirectory, { withFileTypes: true })
      .filter((entry) => entry.isFile() && entry.name.endsWith(".rs"))
      .map((entry) => entry.name.replace(/\.rs$/, "")),
  );
}

describe("feature user story tracker", () => {
  it("keeps a canonical, parseable row for every tracked user story", () => {
    const rows = trackerRows();
    const ids = rows.map((row) => row.id);

    expect(rows).toHaveLength(102);
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids).toEqual([...ids].sort());
    rows.forEach((row) => {
      expectedHeaders.forEach((header) => {
        expect(row[header], `${row.id} ${header} should not be blank`).not.toEqual("");
      });
    });
  });

  it("does not leave current user stories in an untested pending state", () => {
    const rows = trackerRows();

    expect(rows.filter((row) => row.test_status.startsWith("untested_pending"))).toEqual([]);
    expect(rows.filter((row) => row.test_status.startsWith("partial_passed"))).toHaveLength(1);
    expect(rows.filter((row) => row.test_status.startsWith("not_applicable_future_deferred"))).toHaveLength(2);
  });

  it("marks future-only docs as deferred instead of implying a current test failure", () => {
    const rows = trackerRows();
    const futureRows = rows.filter((row) => row.test_layer.includes("future"));

    expect(futureRows.map((row) => row.id)).toEqual(["US-056", "US-086"]);
    futureRows.forEach((row) => {
      expect(row.feature_status).toMatch(/^future_deferred_/);
      expect(row.test_status).toMatch(/^not_applicable_future_deferred_/);
      expect(row.defects).toContain("No current operational defect");
    });
  });

  it("links every current partial story to its unresolved follow-up issue", () => {
    const rows = trackerRows();
    const partialRows = rows.filter((row) => row.test_status.startsWith("partial_passed"));
    const unresolvedIssuesByStory = new Map([
      ["US-057", ["#333", "#541"]],
    ]);

    expect(partialRows.map((row) => row.id)).toEqual([...unresolvedIssuesByStory.keys()]);
    partialRows.forEach((row) => {
      const expectedIssues = unresolvedIssuesByStory.get(row.id) ?? [];
      expectedIssues.forEach((issue) => {
        expect(row.next_verification, `${row.id} next_verification should cite ${issue}`).toContain(issue);
      });
    });
  });

  it("does not treat closed Gemini maintenance issues as current blockers", () => {
    const rows = trackerRows();
    const currentGeminiBlockers = rows
      .filter((row) => row.test_status.startsWith("partial_passed"))
      .filter((row) => /#581|Gemini real-provider rendering/.test(`${row.next_verification} ${row.defects}`))
      .map((row) => row.id);

    expect(currentGeminiBlockers).toEqual([]);
  });

  it("records a dated issue-state audit for every current partial story", () => {
    const rows = trackerRows();
    const partialRows = rows.filter((row) => row.test_status.startsWith("partial_passed"));

    partialRows.forEach((row) => {
      expect(row.notes, `${row.id} notes should record issue state audit evidence`).toMatch(
        /Issue state audited on \d{4}-\d{2}-\d{2}:/,
      );
    });
  });

  it("keeps tested stories tied to explicit verification evidence", () => {
    const rows = trackerRows();
    const evidencePattern = /\b(Verified|Covered by|passed|Coverage|docs site build)\b/i;
    const weakEvidenceRows = rows
      .filter((row) => row.test_status.startsWith("tested_passed"))
      .filter((row) => !evidencePattern.test(row.notes))
      .map((row) => row.id);

    expect(weakEvidenceRows).toEqual([]);
  });

  it("keeps non-Gemini real-provider stories independently verified", () => {
    const rows = trackerRows();
    const rowsById = new Map(rows.map((row) => [row.id, row]));
    const providerStories = [
      {
        id: "US-096",
        provider: "Codex",
        evidence: ["WARDIAN_E2E_REAL_DELIVERY=1", "WARDIAN_E2E_REAL_CODEX_SANDBOX=1", "WARDIAN_E2E_REAL_RENDERING=1"],
      },
      {
        id: "US-098",
        provider: "Claude",
        evidence: ["WARDIAN_E2E_REAL_DELIVERY=1", "WARDIAN_E2E_REAL_RENDERING=1", "bash.exe", "cmd.exe"],
      },
      {
        id: "US-099",
        provider: "OpenCode",
        evidence: ["WARDIAN_E2E_REAL_OPENCODE=1", "WARDIAN_E2E_REAL_DELIVERY=1", "WARDIAN_E2E_REAL_RENDERING=1"],
      },
      {
        id: "US-100",
        provider: "Antigravity",
        evidence: ["WARDIAN_E2E_REAL_ANTIGRAVITY=1", "WARDIAN_E2E_REAL_DELIVERY=1", "WARDIAN_E2E_REAL_RENDERING=1"],
      },
    ];

    providerStories.forEach(({ id, provider, evidence }) => {
      const row = rowsById.get(id);
      expect(row, `${id} should exist`).toBeDefined();
      expect(row?.feature, `${id} should remain the ${provider} provider story`).toContain(provider);
      expect(row?.test_layer, `${id} should remain in the real-provider test layer`).toContain("real-provider");
      expect(row?.test_status, `${id} should stay passed after the non-Gemini provider continuation`).toBe(
        "tested_passed_2026-06-23",
      );
      expect(row?.defects, `${id} should not carry an unresolved ${provider} defect`).toContain(`No ${provider}`);
      evidence.forEach((token) => {
        expect(row?.notes, `${id} notes should retain ${token} evidence`).toContain(token);
      });
    });
  });

  it("does not leave generic next-verification placeholders", () => {
    const rows = trackerRows();
    const genericRows = rows
      .filter((row) => row.next_verification.includes("Rerun listed test layer before PR or after related changes."))
      .map((row) => row.id);

    expect(genericRows).toEqual([]);
  });

  it("keeps concrete-command handoffs backed by command text", () => {
    const rows = trackerRows();
    const commandPattern = /\b(npm|cargo|node|gh|wardian|WARDIAN_|PowerShell|cmd\.exe|bash\.exe)\b/;
    const commandlessRows = rows
      .filter((row) => row.next_verification.includes("Rerun the concrete verification commands recorded in this row"))
      .filter((row) => !commandPattern.test(`${row.next_verification} ${row.notes}`))
      .map((row) => row.id);

    expect(commandlessRows).toEqual([]);
  });

  it("references existing file paths in command evidence", () => {
    const rows = trackerRows();
    const filePathPattern =
      /\b(?:src|e2e|e2e-native|src-tauri|crates|docs|scripts)\/[A-Za-z0-9_.\-/]+\.[A-Za-z0-9]+/g;
    const missing = rows.flatMap((row) => {
      const commandText = `${row.next_verification} ${row.notes}`.replace(/\\/g, "/");
      const paths = [...new Set([...commandText.matchAll(filePathPattern)].map((match) => match[0]))];
      return paths
        .filter((relativePath) => !pathExistsOrIsDirectoryPrefix(relativePath))
        .map((relativePath) => `${row.id}: ${relativePath}`);
    });

    expect(missing).toEqual([]);
  });

  it("references only package scripts that exist", () => {
    const rows = trackerRows();
    const packageJson = JSON.parse(readFileSync(join(repoRoot, "package.json"), "utf8")) as {
      scripts?: Record<string, string>;
    };
    const scripts = new Set(Object.keys(packageJson.scripts ?? {}));
    const missingScripts = [
      ...new Set(
        rows.flatMap((row) =>
          [...`${row.next_verification} ${row.notes}`.matchAll(/\bnpm run ([A-Za-z0-9:_-]+)/g)].map(
            (match) => match[1],
          ),
        ),
      ),
    ]
      .filter((scriptName) => !scripts.has(scriptName))
      .sort();

    expect(missingScripts).toEqual([]);
  });

  it("references only existing cargo packages and integration test targets", () => {
    const rows = trackerRows();
    const packages = new Map(cargoPackageManifests().map((manifest) => [manifest.name, manifest.directory]));
    const packageTestTargets = new Map(
      [...packages].map(([name, directory]) => [name, cargoIntegrationTests(directory)]),
    );
    const rowText = rows.map((row) => ({ id: row.id, text: `${row.next_verification} ${row.notes}` }));

    const missingPackages = [
      ...new Set(
        rowText.flatMap(({ text }) =>
          [...text.matchAll(/\bcargo\s+(?:test|check|clippy)\s+-p\s+([A-Za-z0-9_-]+)/g)].map(
            (match) => match[1],
          ),
        ),
      ),
    ]
      .filter((packageName) => !packages.has(packageName))
      .sort();
    const missingTests = rowText.flatMap(({ id, text }) =>
      [...text.matchAll(/\bcargo\s+test\s+-p\s+([A-Za-z0-9_-]+)([^.;)]*)/g)].flatMap((commandMatch) => {
        const packageName = commandMatch[1];
        const targets = packageTestTargets.get(packageName) ?? new Set<string>();
        return [...commandMatch[2].matchAll(/--test\s+([A-Za-z0-9_-]+)/g)]
          .map((testMatch) => testMatch[1])
          .filter((testName) => !targets.has(testName))
          .map((testName) => `${id}: ${packageName} --test ${testName}`);
      }),
    );

    expect(missingPackages).toEqual([]);
    expect(missingTests).toEqual([]);
  });

  it("references only existing code and documentation evidence paths", () => {
    const rows = trackerRows();
    const missing = rows.flatMap((row) =>
      evidencePaths(row)
        .filter((relativePath) => !pathExistsOrIsDirectoryPrefix(relativePath))
        .map((relativePath) => `${row.id}: ${relativePath}`),
    );

    expect(missing).toEqual([]);
  });

  it("covers every authored spec file except the reusable template", () => {
    const rows = trackerRows();
    const documentedSpecs = documentedMarkdownPaths(rows);
    const missingSpecs = markdownFiles(join(repoRoot, "docs", "specs"))
      .map((absolutePath) => absolutePath.slice(repoRoot.length + 1))
      .filter((relativePath) => relativePath !== "docs/specs/template.md")
      .filter((relativePath) => !documentedSpecs.has(relativePath));

    expect(missingSpecs).toEqual([]);
  });

  it("covers every authored feature documentation file outside reference/template folders", () => {
    const rows = trackerRows();
    const documentedDocs = documentedMarkdownPaths(rows);
    const missingDocs = markdownFiles(join(repoRoot, "docs"))
      .map((absolutePath) => absolutePath.slice(repoRoot.length + 1))
      .filter((relativePath) => !isNonStoryReferenceDoc(relativePath))
      .filter((relativePath) => !documentedDocs.has(relativePath));

    expect(missingDocs).toEqual([]);
  });
});
