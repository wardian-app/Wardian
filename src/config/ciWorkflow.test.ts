import { describe, expect, it } from "vitest";
import ciWorkflow from "../../.github/workflows/ci.yml?raw";

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
});
