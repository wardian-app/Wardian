import { expect, test } from "@playwright/test";

test("remote mobile shell renders one-column agents and sends a CSRF-protected prompt", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });

  const actionRequests: Array<{ headers: Record<string, string>; body: unknown }> = [];

  await page.route("**/remote/api/session", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        csrf_nonce: "csrf-e2e",
        expires_at: "2099-05-21T08:05:00.000Z",
        absolute_expires_at: "2099-05-21T20:00:00.000Z",
      }),
    });
  });
  await page.route("**/remote/api/agents", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        agents: [
          {
            session_id: "agent-1",
            session_name: "Remote Coder",
            agent_class: "Coder",
            provider: "codex",
            workspace: "<absolute-workspace-path>",
            status: "Idle",
            latest_text: "Ready",
          },
        ],
      }),
    });
  });
  await page.route("**/remote/api/workflows", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ workflows: [] }) });
  });
  await page.route("**/remote/api/agents/agent-1/chat", async (route) => {
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ events: [] }) });
  });
  await page.route("**/remote/api/agents/agent-1/terminal", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        snapshot: {
          cursor: "agent-1:0000000000000001",
          text: "terminal ready from e2e",
          truncated: false,
          omitted_bytes: 0,
        },
      }),
    });
  });
  await page.route("**/remote/api/agents/action", async (route) => {
    actionRequests.push({
      headers: route.request().headers(),
      body: JSON.parse(route.request().postData() ?? "{}"),
    });
    await route.fulfill({ contentType: "application/json", body: JSON.stringify({ ok: true }) });
  });

  await page.goto("/remote", { waitUntil: "domcontentloaded" });
  await expect(page.locator('[data-testid="remote-mobile-app"]')).toBeVisible();
  await expect(page.getByText("Remote Coder")).toBeVisible();
  await expect(page.locator('[data-testid="remote-agent-list"]')).toHaveClass(/grid-cols-1/);

  await page.getByRole("button", { name: "Open Remote Coder details" }).click();
  await expect(page.locator('[data-testid="remote-agent-detail"]')).toBeVisible();
  await expect(page.getByRole("button", { name: "Terminal", exact: true })).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByText("terminal ready from e2e")).toBeVisible();
  await page.getByLabel("Prompt Remote Coder").fill("status please");
  await page.getByRole("button", { name: "Send prompt" }).click();

  await expect.poll(() => actionRequests.length).toBe(1);
  expect(actionRequests[0]).toMatchObject({
    headers: {
      "x-wardian-csrf": "csrf-e2e",
    },
    body: {
      action: "send_prompt",
      target: "agent-1",
      prompt: "status please",
    },
  });
});
