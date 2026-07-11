import { describe, expect, it } from "vitest";
import ciWorkflow from "../../.github/workflows/ci.yml?raw";
import codecovConfig from "../../.codecov.yml?raw";
import readme from "../../README.md?raw";

function jobDefinition(jobName: string) {
  const lines = ciWorkflow.split(/\r?\n/);
  const start = lines.indexOf(`  ${jobName}:`);
  if (start === -1) {
    throw new Error(`CI job ${jobName} is not defined`);
  }

  const nextJobOffset = lines
    .slice(start + 1)
    .findIndex((line) => /^  [a-z0-9-]+:$/.test(line));
  const end = nextJobOffset === -1 ? lines.length : start + 1 + nextJobOffset;
  return lines.slice(start, end).join("\n");
}

describe("CI workflow contract", () => {
  it("gates frontend, backend, documentation, screenshots, and workbench cutover", () => {
    const frontend = jobDefinition("frontend-quality");
    const backend = jobDefinition("backend-windows");
    const backendCoverage = jobDefinition("backend-linux-coverage");
    const docs = jobDefinition("docs-quality");

    expect(ciWorkflow).toMatch(/pull_request:\s+branches: \[main\]/);
    for (const requiredJob of [frontend, backend, backendCoverage, docs]) {
      expect(requiredJob).not.toMatch(/^    if:/m);
    }
    expect(frontend).toContain("run: npm run lint");
    expect(frontend).toContain("run: npm run test");
    expect(frontend).toContain("run: npm run build");
    expect(frontend).toContain("run: npm run check:workbench-cutover");
    expect(frontend).toMatch(
      /- name: Require frontend screenshot evidence\s+if: github\.event_name == 'pull_request'\s+run: npm run check:frontend-screenshot/,
    );
    expect(backend).toContain("cargo clippy --workspace -- -D warnings");
    expect(backend).toContain("cargo test --workspace -- --test-threads=1");
    expect(backend).toContain("cargo check --workspace");
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
    expect(native).not.toMatch(/^    if:/m);
    expect(native).toContain(
      "WARDIAN_HOME: ${{ runner.temp }}\\wardian-e2e-native-workbench",
    );
    expect(native).toContain(
      "WARDIAN_E2E_NATIVE_HOME: ${{ runner.temp }}\\wardian-e2e-native-workbench",
    );
    expect(native).toContain("run: npm run setup:e2e:native");
    expect(native).toContain("run: npm run tauri -- build --debug --no-bundle");
    expect(native).toContain("npm run test:e2e:native:fast --");
    expect(native).toContain("e2e-native/tests/workbench-persistence-native.test.mjs");
    expect(native).toContain("e2e-native/tests/terminal-presentation-broker-native.test.mjs");
    expect(native).toContain("e2e-native/tests/workbench-runtime-lifecycle-native.test.mjs");
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
