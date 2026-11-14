import { expect, test } from "@playwright/test";
import { mkdirSync } from "node:fs";
import path from "node:path";
import { installWorkbenchIpcMock } from "../fixtures/workbenchIpcMock";

for (const width of [1920, 1024, 800, 740]) {
  test(`remote access description stays readable at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1080 });
    await installWorkbenchIpcMock(page, { responses: {
      load_remote_access_status: "enabled",
      load_remote_gateway_config: {
        schema_version: 1, enabled: true, canonical_origin: "https://gateway.example",
        client_origin_allowlist: ["https://companion-with-a-long-hostname.example"],
        loopback_host: "127.0.0.1", loopback_port: 41241,
        gateway_identity_public_key: "example-key", gateway_identity_fingerprint: "example-fingerprint",
      },
      list_remote_devices: [], list_pending_remote_pairing_requests: [],
      load_remote_setup_check: { overall_status: "ready", checks: [] },
    } });
    await page.goto("/");
    await page.locator('[data-testid="sidebar-tab-settings"]').click();
    const dialog = page.getByRole("dialog", { name: "Settings" });
    await dialog.getByRole("button", { name: "Remote Access", exact: true }).click();
    const section = dialog.getByRole("region", { name: "Remote Access", exact: true }).last();
    const description = section.getByText("Pair a phone for full remote control,", { exact: false });
    await expect(description).toBeAttached();
    const descriptionBox = await description.boundingBox();
    const sectionBox = await section.boundingBox();
    // A desktop viewport can still contain a narrow settings pane. Guard the
    // rendered paragraph width rather than a CSS class or viewport breakpoint.
    expect(descriptionBox!.width).toBeGreaterThanOrEqual(Math.min(280, sectionBox!.width - 32));
    await expect(section.getByRole("group", { name: "Phone pairing", exact: true })).toBeVisible();
    await expect(section.getByRole("group", { name: "Companion client", exact: true })).toBeVisible();
    expect(await section.evaluate((element) => element.scrollWidth - element.clientWidth)).toBeLessThanOrEqual(1);
    if (width === 1920 || width === 740) {
      const evidence = path.join("e2e", "screenshots", "remote-access-layout", new Date().toISOString().replace(/[:.]/g, "-"));
      mkdirSync(evidence, { recursive: true });
      await dialog.screenshot({ path: path.join(evidence, `${width === 1920 ? "desktop" : "narrow"}.png`) });
    }
  });
}

test("operator can create and copy an origin-bound HUD offer", async ({ page }) => {
  const offer = {
    gateway_origin: "https://gateway.example",
    credential_kind: "client", audience: "wardian_remote_client_v1",
    client_origin: "https://hud.example", read_grants: ["agents:read", "chat:read"],
    pairing_offer_id: "example-offer", nonce: "example-nonce",
    expires_at: "2099-01-01T00:00:00.000Z", server_identity_fingerprint: "example-fingerprint",
  };
  await installWorkbenchIpcMock(page, { responses: {
    load_remote_access_status: "enabled",
    load_remote_gateway_config: {
      schema_version: 1, enabled: true, canonical_origin: "https://gateway.example",
      client_origin_allowlist: ["https://hud.example"], loopback_host: "127.0.0.1", loopback_port: 41241,
      gateway_identity_public_key: "example-key", gateway_identity_fingerprint: "example-fingerprint",
    },
    list_remote_devices: [], list_pending_remote_pairing_requests: [],
    load_remote_setup_check: { overall_status: "ready", checks: [] },
    create_remote_client_pairing_offer: offer,
  } });
  await page.goto("/");
  await page.locator('[data-testid="sidebar-tab-settings"]').click();
  const dialog = page.getByRole("dialog", { name: "Settings" });
  await dialog.getByRole("button", { name: "Remote Access", exact: true }).click();
  await dialog.getByRole("button", { name: "Create client pairing code", exact: true }).click();
  await expect(dialog.getByRole("textbox", { name: "Client pairing JSON" })).toHaveValue(JSON.stringify(offer));
  await expect(dialog.getByRole("combobox", { name: "Paired client origin" })).toHaveValue("https://hud.example");
  await dialog.getByRole("textbox", { name: "Client pairing JSON" }).scrollIntoViewIfNeeded();
  const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
  const evidence = path.join("e2e", "screenshots", "remote-client-pairing", timestamp);
  mkdirSync(evidence, { recursive: true });
  await dialog.screenshot({ path: path.join(evidence, "hud-pairing-offer.png") });
});
