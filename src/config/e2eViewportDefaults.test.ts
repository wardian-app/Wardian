import { readFileSync } from "node:fs";
import { resolve } from "node:path";

describe("browser E2E viewport defaults", () => {
  test("uses a fullscreen desktop viewport by default with env overrides", () => {
    const config = readFileSync(resolve("e2e/playwright.config.ts"), "utf8");

    expect(config).toContain("WARDIAN_E2E_VIEWPORT_WIDTH");
    expect(config).toContain("WARDIAN_E2E_VIEWPORT_HEIGHT");
    expect(config).toContain('?? "1920"');
    expect(config).toContain('?? "1080"');
    expect(config).toMatch(/viewport:\s*e2eViewport/);
  });
});
