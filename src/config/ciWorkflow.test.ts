import { describe, expect, it } from "vitest";
import ciWorkflow from "../../.github/workflows/ci.yml?raw";
import codecovConfig from "../../.codecov.yml?raw";
import readme from "../../README.md?raw";

describe("CI workflow contract", () => {
  it("runs test jobs on main branch pushes as well as pull requests", () => {
    expect(ciWorkflow).not.toContain("Unit Tests\n        if: github.event_name == 'pull_request'");
    expect(ciWorkflow).not.toContain("backend-windows:\n    name: Backend (Windows - Lint, Test & Check)\n    runs-on: windows-latest\n    if: github.event_name == 'pull_request'");
    expect(ciWorkflow).not.toContain("backend-linux-coverage:\n    name: Backend (Linux - Test & Coverage)\n    runs-on: ubuntu-latest\n    if: github.event_name == 'pull_request'");
    expect(ciWorkflow).toContain("run: npm run test");
    expect(ciWorkflow).toContain("cargo clippy --workspace -- -D warnings");
    expect(ciWorkflow).toContain("cargo test --workspace");
    expect(ciWorkflow).toContain("cargo check --workspace");
    expect(ciWorkflow).toContain("cargo llvm-cov --workspace --lcov --output-path coverage/rust-lcov.info");
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
