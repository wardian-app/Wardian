import { describe, expect, it } from "vitest";
import ciWorkflow from "../../.github/workflows/ci.yml?raw";
import docsWorkflow from "../../.github/workflows/docs.yml?raw";
import codecovConfig from "../../.codecov.yml?raw";
import readme from "../../README.md?raw";
import { readVerificationPlan } from "../../scripts/verify-ci.mjs";

function jobDefinition(jobName: string) {
  const lines = ciWorkflow.split(/\r?\n/);
  const start = lines.indexOf(`  ${jobName}:`);
  if (start === -1) {
    throw new Error(`CI job ${jobName} is not defined`);
  }

  const nextJobOffset = lines
    .slice(start + 1)
    .findIndex((line) => /^ {2}[a-z0-9-]+:$/.test(line));
  const end = nextJobOffset === -1 ? lines.length : start + 1 + nextJobOffset;
  return lines.slice(start, end).join("\n");
}

describe("CI workflow contract", () => {
  it.each([
    {
      name: "CI",
      workflow: ciWorkflow,
      triggers: `on:
  push:
    branches: [main]
  pull_request:
`,
    },
    {
      name: "docs",
      workflow: docsWorkflow,
      triggers: `on:
  push:
    branches: [main]
    paths:
      - "docs/**"
      - "package.json"
      - "package-lock.json"
      - ".github/workflows/docs.yml"
  pull_request:
    paths:
      - "docs/**"
      - "package.json"
      - "package-lock.json"
      - ".github/workflows/docs.yml"
  workflow_dispatch:
`,
    },
  ])("routes $name pull requests to every base while preserving push and path filters", ({ workflow, triggers }) => {
    // Pin the complete event block: a PR base filter silently excludes stacked PRs.
    const eventBlock = workflow.replace(/\r\n/g, "\n").match(/^on:\n(?:[ \t].*\n|\n)*/m)?.[0];
    expect(eventBlock?.trimEnd()).toBe(triggers.trimEnd());
  });

  it("keeps Pages configuration, artifact upload, and deployment disabled for pull requests", () => {
    expect(docsWorkflow).toMatch(
      /- name: Configure Pages\s+if: github\.event_name != 'pull_request'\s+uses: actions\/configure-pages@v5/,
    );
    expect(docsWorkflow).toMatch(
      /- name: Upload Pages artifact\s+if: github\.event_name != 'pull_request'\s+uses: actions\/upload-pages-artifact@v3/,
    );
    expect(docsWorkflow).toMatch(
      / {2}deploy:\s+name: Deploy docs\s+needs: build\s+runs-on: ubuntu-latest\s+if: github\.event_name != 'pull_request'/,
    );
    expect(docsWorkflow).toContain("uses: actions/deploy-pages@v4");
  });

  it("gates frontend, backend, documentation, screenshots, and workbench cutover", () => {
    const frontend = jobDefinition("frontend-quality");
    const backend = jobDefinition("backend-windows");
    const backendCoverage = jobDefinition("backend-linux-coverage");
    const docs = jobDefinition("docs-quality");

    for (const requiredJob of [frontend, backend, backendCoverage, docs]) {
      expect(requiredJob).not.toMatch(/^ {4}if:/m);
    }
    expect(frontend).toContain("run: npm run lint");
    expect(frontend).toContain("run: npm run test");
    expect(frontend).toContain("run: npm run build");
    expect(frontend).toContain("run: npm run check:workbench-cutover");
    expect(frontend).toMatch(
      /- name: Require frontend screenshot evidence\s+if: github\.event_name == 'pull_request'/,
    );
    expect(frontend).toContain("GH_TOKEN: ${{ github.token }}");
    expect(frontend).toContain(
      'PR_BODY="$(gh api "repos/${GITHUB_REPOSITORY}/pulls/${{ github.event.pull_request.number }}" --jq .body)"',
    );
    expect(frontend).toContain(
      "npm run check:frontend-screenshot -- origin/${{ github.base_ref }} HEAD",
    );
    expect(readVerificationPlan(backend).map(({ command }) => command)).toEqual([
      "cargo clippy --workspace --all-targets -- -D warnings",
      "cargo fmt --all -- --check",
      "cargo test --workspace --all-targets -- --test-threads=1",
      "cargo test --workspace --doc -- --test-threads=1",
      "cargo check --workspace",
    ]);
    expect(backendCoverage).toContain(
      "cargo llvm-cov --workspace --lcov --output-path coverage/rust-lcov.info",
    );
    expect(docs).toContain("run: npm run docs:check-llms");
    expect(docs).toContain("run: npm run docs:build");
  });

  it("runs focused and full browser workbench coverage on pull requests", () => {
    const browser = jobDefinition("browser-workbench");

    expect(browser).not.toContain("github.event_name == 'push'");
    expect(browser).not.toContain("github.event_name == 'pull_request'");
    expect(browser).toContain("run: npm run test:e2e:workbench");
    expect(browser).toContain("run: npm run test:e2e");
    expect(browser).toContain("WARDIAN_HOME: ${{ runner.temp }}\\wardian-e2e-browser");
    expect(browser).toMatch(/if: failure\(\)[\s\S]*name: e2e-results/);
  });

  it("runs the required native persistence, broker, and lifecycle smoke on Windows", () => {
    const native = jobDefinition("native-workbench-smoke");

    expect(native).toContain("runs-on: windows-latest");
    expect(native).not.toMatch(/^ {4}if:/m);
    expect(native).toContain(
      "WARDIAN_HOME: ${{ github.workspace }}\\.tmp\\e2e-native\\wardian-e2e-native-workbench",
    );
    expect(native).toContain(
      "WARDIAN_E2E_NATIVE_HOME: ${{ github.workspace }}\\.tmp\\e2e-native\\wardian-e2e-native-workbench",
    );
    expect(native).toContain("run: npm run setup:e2e:native");
    expect(native).toContain("run: npm run tauri -- build --debug --no-bundle");
    // The job selects a tier rather than naming files, so a new native test
    // joins CI by declaring `// @tier ci`.
    expect(native).toContain("run: npm run test:e2e:native:ci");
    expect(native).not.toMatch(/e2e-native\/tests\/\S+\.test\.mjs/);
    expect(native).not.toContain("${{ runner.temp }}\\wardian-e2e-native-workbench");
    expect(native.match(/WARDIAN_HOME: \$\{\{ github\.workspace \}\}\\\.tmp\\e2e-native\\wardian-e2e-native-workbench/g))
      .toHaveLength(2);
    expect(native.match(/WARDIAN_E2E_NATIVE_HOME: \$\{\{ github\.workspace \}\}\\\.tmp\\e2e-native\\wardian-e2e-native-workbench/g))
      .toHaveLength(2);
    expect(native).toContain(
      "${{ github.workspace }}\\.tmp\\e2e-native\\wardian-e2e-native-workbench",
    );
    expect(native).toMatch(/if: failure\(\)[\s\S]*name: native-workbench-results/);
  });

  it("uploads frontend coverage on main branch pushes for Codecov branch badges", () => {
    expect(ciWorkflow).toMatch(/- name: Coverage Report\s+run: npm run test:coverage/);
    expect(ciWorkflow).toMatch(
      /- name: Upload Frontend Coverage\s+uses: codecov\/codecov-action@v7\s+with:\s+files: \.\/coverage\/lcov\.info\s+flags: frontend/,
    );
  });

  it("advertises one combined Codecov badge in the README", () => {
    const badgeUrls = readme.match(
      /https:\/\/codecov\.io\/gh\/wardian-app\/Wardian\/branch\/main\/graph\/badge\.svg[^\)]*/g,
    );

    expect(badgeUrls).toHaveLength(1);
    expect(badgeUrls?.[0]).not.toContain("flag=");
    expect(readme).not.toContain("flag=frontend");
    expect(readme).not.toContain("flag=backend");
  });

  it("declares frontend and backend Codecov flags with scoped paths", () => {
    expect(codecovConfig).toMatch(/flags:\s+frontend:\s+paths:\s+- src\//);
    expect(codecovConfig).toMatch(/backend:\s+paths:\s+- src-tauri\/\s+- crates\//);
  });
});
