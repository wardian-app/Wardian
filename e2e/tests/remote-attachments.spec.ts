import fs from "node:fs";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";

// Browser-only HTTP fixtures prove the compact composer and wire payload, not
// native filesystem admission, provider delivery, or clipboard behavior.
const photo = {
  name: "photo #1.png", mimeType: "image/png",
  buffer: Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAgElEQVR4nNXOQREAIAzAsFIhaEInKhGxB9coyNr3UCZxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEidxEufvwNQDB60BmAyWdhQAAAAASUVORK5CYII=", "base64"),
};
const agent = { session_id: "attachment-fixture", session_name: "Attachment Fixture", agent_class: "Coder",
  provider: "codex", workspace: "<agent-workspace>", status: "Idle", latest_text: null };
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const submissionKey = new RegExp(`^\\d+\\.${uuid.source.slice(1, -1)}$`);

async function capture(page: Page, name: string) {
  const directory = process.env.WARDIAN_REMOTE_ATTACHMENTS_SCREENSHOT_DIR;
  if (!directory) return;
  fs.mkdirSync(directory, { recursive: true });
  await page.screenshot({ path: path.join(directory, `${name}.png`), animations: "disabled" });
}

async function install(page: Page, options: { theme?: "light" | "dark"; holdPhoto?: boolean; failText?: boolean; uncertain?: boolean; lostSubmission?: "missing" | "expired" } = {}) {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.addInitScript(() => localStorage.setItem("wardian.remote.agentDefaultViewMode", "chat"));
  if (options.theme) await page.addInitScript((theme) => localStorage.setItem("wardian-settings", JSON.stringify({
    state: { theme }, version: 2,
  })), options.theme);
  let release!: () => void;
  const held = new Promise<void>((resolve) => { release = resolve; });
  const uploads: Array<{ id: string; name: string; bytes: Buffer; csrf?: string; media_type?: string }> = [];
  const deleted: string[] = [];
  const submissions: Array<{ target: string; prompt: string; attachment_ids: string[]; submission_id: string }> = [];
  const statusReads: string[] = [];
  // This fixture models ledger deduplication only; native/HTTP tests own proof
  // that the real gateway fences provider effects when the original arrives late.
  const acceptedKeys = new Set<string>();
  const completed = new Map<string, { upload_id: string; name: string; size_bytes: number; media_type: string; expires_at: string }>();
  let textFailed = false;
  await page.route("**/remote/api/**", async (route) => {
    const request = route.request();
    const endpoint = new URL(request.url()).pathname;
    const uploadId = endpoint.match(/\/attachments\/([^/]+)$/)?.[1];
    if (uploadId) {
      if (request.method() === "DELETE") {
        expect(request.headers()["x-wardian-csrf"]).toBe("fixture-csrf");
        deleted.push(uploadId); completed.delete(uploadId);
        await route.fulfill({ json: { ok: true } }); return;
      }
      if (request.method() === "GET") {
        await route.fulfill(completed.has(uploadId) ? { json: completed.get(uploadId) }
          : { status: 404, json: { code: "attachment_not_found" } }); return;
      }
      expect(request.method()).toBe("POST");
      expect(uploadId).toMatch(uuid);
      const headers = request.headers();
      const name = decodeURIComponent(headers["x-wardian-filename"]);
      const bytes = request.postDataBuffer();
      expect(bytes).not.toBeNull();
      uploads.push({ id: uploadId, name, bytes: bytes!, csrf: headers["x-wardian-csrf"], media_type: headers["content-type"] });
      if (name === photo.name && options.holdPhoto) await held;
      if (name.endsWith(".txt") && options.failText && !textFailed) {
        textFailed = true;
        await route.fulfill({ status: 429, headers: { "Retry-After": "0" }, json: { code: "attachment_quota" } }); return;
      }
      const metadata = { upload_id: uploadId, name, size_bytes: bytes!.length,
        media_type: headers["content-type"], expires_at: "2099-01-01T00:00:00Z" };
      completed.set(uploadId, metadata);
      await route.fulfill({ json: metadata }); return;
    }
    if (endpoint.endsWith("/action")) {
      expect(request.headers()["x-wardian-csrf"]).toBe("fixture-csrf");
      const body = request.postDataJSON();
      expect(Object.keys(body).sort()).toEqual(["action", "attachment_ids", "prompt", "submission_id", "target"]);
      expect(body.action).toBe("send_prompt"); expect(body.target).toBe(agent.session_id);
      expect(body.submission_id).toMatch(submissionKey);
      const issued = Number(body.submission_id.split(".")[0]);
      expect(Math.abs(Date.now() - issued)).toBeLessThan(60_000);
      expect(body.attachment_ids.length).toBeGreaterThan(0);
      for (const id of body.attachment_ids) { expect(id).toMatch(uuid); expect(completed.has(id)).toBe(true); }
      // Only server-owned IDs cross the action boundary; no client paths or file bytes.
      submissions.push(body);
      if (options.lostSubmission && submissions.length === 1) { await route.abort("failed"); return; }
      acceptedKeys.add(body.submission_id);
      await route.fulfill({ json: { ok: true, submission_id: body.submission_id,
        state: options.uncertain ? "submitted_unconfirmed" : "accepted" } }); return;
    }
    const submissionId = endpoint.match(/\/attachment-submissions\/([^/]+)$/)?.[1];
    if (submissionId) {
      expect(request.method()).toBe("GET"); statusReads.push(submissionId);
      if (options.lostSubmission && !acceptedKeys.has(submissionId)) {
        await route.fulfill({ status: options.lostSubmission === "missing" ? 404 : 410,
          json: { code: options.lostSubmission === "missing" ? "attachment_not_found" : "submission_expired" } }); return;
      }
      await route.fulfill({ json: { ok: true, submission_id: submissionId, state: "accepted" } }); return;
    }
    if (endpoint.endsWith("/chat")) {
      await route.fulfill({ json: { session_id: agent.session_id, conversation_id: "fixture-conversation", generation: "fixture-generation",
        source_epoch: null, revision: "fixture", unchanged: false, reset: false, progress: "ready", aliases: [], removed_ids: [],
        detail: null, bytes_read: 0, records_decoded: 0, events: [], next_before: null } }); return;
    }
    const responses: Record<string, unknown> = {
      "/remote/api/session": { csrf_nonce: "fixture-csrf", expires_at: "2099-01-01T00:00:00Z", absolute_expires_at: "2099-01-01T00:00:00Z" },
      "/remote/api/agents": { agents: [agent] }, "/remote/api/watchlists": { watchlists: [], teams: [], prefs: null },
      "/remote/api/queue": { items: [] }, "/remote/api/automations": { automations: [] },
      "/remote/api/ws-ticket": { ticket: "fixture-ticket", expires_at: "2099-01-01T00:00:00Z" },
    };
    await route.fulfill({ json: responses[endpoint] ?? {} });
  });
  await page.routeWebSocket("**/remote/api/status-stream", (socket) => {
    socket.onMessage(() => socket.send(JSON.stringify({ type: "agent_status", agents: [agent] })));
  });
  await page.goto("/remote", { waitUntil: "domcontentloaded" });
  await page.getByRole("button", { name: "Open Attachment Fixture details" }).click();
  await expect(page.getByRole("textbox", { name: "Prompt Attachment Fixture" })).toBeVisible();
  return { uploads, deleted, submissions, statusReads, acceptedKeys, release,
    admitLateOriginal() { acceptedKeys.add(submissions[0].submission_id); } };
}

test.describe("mobile remote attachments", () => {
  test.setTimeout(45_000);
  for (const theme of ["light", "dark"] as const) {
    test(`${theme} unified composer grows with equal controls and internal attachments`, async ({ page }) => {
      const traffic = await install(page, { theme });
      const field = page.getByTestId("remote-composer-field");
      const input = page.getByRole("textbox", { name: "Prompt Attachment Fixture" });
      const attach = field.getByRole("button", { name: "Attach file", exact: true });
      const send = field.getByRole("button", { name: "Send prompt", exact: true });
      await expect(field).toBeVisible();
      await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
      const geometry = async () => {
        const [container, leading, trailing, text] = await Promise.all([
          field.boundingBox(), attach.boundingBox(), send.boundingBox(), input.boundingBox(),
        ]);
        expect(leading!.width).toBeGreaterThanOrEqual(44);
        expect(leading!.height).toBeGreaterThanOrEqual(44);
        expect(trailing!.width).toBe(leading!.width);
        expect(trailing!.height).toBe(leading!.height);
        expect(Math.abs(leading!.y - trailing!.y)).toBeLessThanOrEqual(1);
        expect(Math.abs(text!.y + text!.height - trailing!.y - trailing!.height)).toBeLessThanOrEqual(1);
        expect(container!.x).toBeLessThan(leading!.x);
        expect(container!.x + container!.width).toBeGreaterThan(trailing!.x + trailing!.width);
        await expect(attach).toHaveCSS("border-left-width", "0px");
        await expect(input).toHaveCSS("border-left-width", "0px");
        const radius = await send.evaluate((element) => parseFloat(getComputedStyle(element).borderRadius));
        expect(radius).toBeGreaterThanOrEqual(trailing!.width / 2);
        expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
        return text!;
      };
      await expect(send).toBeDisabled();
      const empty = await geometry();
      expect(empty.height).toBe(44);
      const muted = await send.evaluate((element) => getComputedStyle(element).backgroundColor);
      const mutedForeground = await send.evaluate((element) => getComputedStyle(element).color);
      await capture(page, `${theme}-composer-empty`);
      await input.fill("Please inspect the sample data.");
      await expect(send).toBeEnabled();
      const colors = await page.evaluate(() => {
        const probe = document.createElement("span");
        probe.style.backgroundColor = "var(--color-wardian-accent)";
        probe.style.color = "var(--color-wardian-text-muted)";
        document.body.append(probe);
        const style = getComputedStyle(probe);
        const colors = { accent: style.backgroundColor, muted: style.color };
        probe.remove(); return colors;
      });
      await expect(send).toHaveCSS("background-color", colors.accent);
      expect(muted).not.toBe(colors.accent);
      expect(mutedForeground).toBe(colors.muted);
      await geometry();
      await capture(page, `${theme}-composer-typed`);
      await input.fill("Review sample rows.\nSummarize the totals.\nKeep the reply brief.");
      expect((await geometry()).height).toBeGreaterThan(empty.height);
      await capture(page, `${theme}-composer-multiline`);
      await input.fill("Sample line.\n".repeat(20));
      expect((await geometry()).height).toBeLessThanOrEqual(160);
      expect(await input.evaluate((element) => element.scrollHeight > element.clientHeight)).toBe(true);
      await input.fill("Please inspect the attached sample.");
      await attach.click();
      await page.getByRole("button", { name: "Files", exact: true }).click();
      await page.getByLabel("Choose files").setInputFiles(photo);
      await expect(field.getByText("Temporary · ready")).toBeVisible();
      const chips = await field.getByLabel("Temporary attachments").boundingBox();
      expect(chips!.y + chips!.height).toBeLessThanOrEqual((await geometry()).y);
      await capture(page, `${theme}-composer-attachment`);
      await send.click();
      await expect(input).toHaveValue("");
      await expect(field.getByLabel("Temporary attachments")).toHaveCount(0);
      await expect(send).toBeDisabled();
      expect((await geometry()).height).toBe(empty.height);
      expect(traffic.submissions).toHaveLength(1);
      expect(traffic.submissions[0]).toMatchObject({ prompt: "Please inspect the attached sample.", attachment_ids: [traffic.uploads[0].id] });
    });
  }
  test("Markdown reported as plain text uploads with canonical Content-Type", async ({ page }) => {
    const traffic = await install(page);
    const buffer = Buffer.from("# UTF-8 Markdown\n");
    await page.getByLabel("Choose files").setInputFiles({ name: "note.md", mimeType: "text/plain", buffer });
    await expect(page.getByText("Temporary · ready")).toBeVisible();
    expect(traffic.uploads).toHaveLength(1);
    expect(traffic.uploads[0].media_type).toBe("text/markdown");
    expect(traffic.uploads[0].bytes.equals(buffer)).toBe(true);
    expect(traffic.submissions).toHaveLength(0);
  });
  test("ready image, removal and attachment-only submission use binary uploads and IDs", async ({ page }) => {
    const traffic = await install(page);
    await page.getByRole("button", { name: "Attach file", exact: true }).click();
    await expect(page.getByRole("button", { name: "Files", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Camera", exact: true })).toBeVisible();
    await expect(page.getByLabel("Take photo")).toHaveAttribute("capture", "environment");
    await page.getByRole("button", { name: "Files", exact: true }).click();
    await page.getByLabel("Choose files").setInputFiles(photo);
    await expect(page.getByText("Temporary · ready")).toBeVisible();
    await expect(page.locator('img[src^="blob:"]')).toBeVisible();
    expect(traffic.uploads[0].bytes.equals(photo.buffer)).toBe(true);
    expect(traffic.uploads[0]).toMatchObject({ name: photo.name, csrf: "fixture-csrf", media_type: "image/png" });
    for (const label of ["Attach file", `Remove ${photo.name}`, "Send prompt"]) {
      const box = await page.getByRole("button", { name: label, exact: true }).boundingBox();
      expect(box!.width).toBeGreaterThanOrEqual(40); expect(box!.height).toBeGreaterThanOrEqual(40);
    }
    await capture(page, "mobile-ready-image");
    await page.getByRole("button", { name: `Remove ${photo.name}`, exact: true }).click();
    await expect(page.getByLabel("Temporary attachments")).toHaveCount(0);
    await expect.poll(() => traffic.deleted.length).toBe(1);
    await capture(page, "mobile-removed-compact");
    await page.getByLabel("Choose files").setInputFiles(photo);
    await expect(page.getByText("Temporary · ready")).toBeVisible();
    await page.getByRole("button", { name: "Send prompt", exact: true }).click();
    await expect(page.getByLabel("Temporary attachments")).toHaveCount(0);
    expect(traffic.submissions).toHaveLength(1);
    expect(traffic.submissions[0]).toMatchObject({ prompt: "Please inspect the attached files.", attachment_ids: [traffic.uploads[1].id] });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });

  test("pending validation and explicit upload retry block Send", async ({ page }) => {
    const traffic = await install(page, { holdPhoto: true, failText: true });
    try {
      await page.getByLabel("Choose files").setInputFiles([photo, { name: "note.txt", mimeType: "text/plain", buffer: Buffer.from("UTF-8 note") }]);
      // Chromium pauses intercepted requests before transfer progress. Keep the
      // real pending XHR visible; unit transport tests cover the 100% fence.
      await expect(page.getByRole("progressbar", { name: `Uploading ${photo.name}` })).toBeVisible();
      await expect(page.getByRole("button", { name: "Retry note.txt" })).toBeVisible();
      await expect(page.getByRole("button", { name: "Send prompt", exact: true })).toBeDisabled();
      await capture(page, "mobile-progress-error");
      traffic.release();
      await expect(page.getByText("Temporary · ready")).toBeVisible();
      await page.getByRole("button", { name: "Retry note.txt" }).click();
      await expect(page.getByText("Temporary · ready")).toHaveCount(2);
      expect(traffic.uploads.filter((upload) => upload.name === "note.txt").map((upload) => upload.id))
        .toEqual([traffic.uploads[1].id, traffic.uploads[1].id]);
      expect(traffic.submissions).toHaveLength(0);
      await expect(page.getByRole("button", { name: "Send prompt", exact: true })).toBeEnabled();
    } finally { traffic.release(); }
  });

  test("uncertain send recovers accepted state without replay or a chat receipt", async ({ page }) => {
    const traffic = await install(page, { uncertain: true });
    await page.getByLabel("Choose files").setInputFiles(photo);
    await expect(page.getByText("Temporary · ready")).toBeVisible();
    await page.getByRole("textbox").fill("Inspect this image.");
    await page.getByRole("button", { name: "Send prompt", exact: true }).click();
    await expect(page.getByRole("button", { name: "Send prompt", exact: true })).toBeDisabled();
    await expect(page.getByRole("textbox")).toHaveValue("Inspect this image.");
    await page.getByRole("button", { name: "Check delivery status", exact: true }).click();
    await expect(page.getByLabel("Temporary attachments")).toHaveCount(0);
    await expect(page.getByRole("textbox")).toHaveValue("");
    expect(traffic.submissions).toHaveLength(1);
    expect(traffic.statusReads).toEqual([traffic.submissions[0].submission_id]);
  });
  test("a lost request and 404 allow only explicit same-key retry even after a late original", async ({ page }) => {
    const traffic = await install(page, { lostSubmission: "missing" });
    await page.getByLabel("Choose files").setInputFiles([photo, { name: "note.md", mimeType: "text/plain", buffer: Buffer.from("# second") }]);
    await expect(page.getByText("Temporary · ready")).toHaveCount(2);
    await page.getByRole("textbox").fill("Inspect in this order.");
    await page.getByRole("button", { name: "Send prompt", exact: true }).click();
    await page.getByRole("button", { name: "Check delivery status", exact: true }).click();
    const retry = page.getByRole("button", { name: "Retry send", exact: true });
    await expect(retry).toBeVisible();
    expect(traffic.submissions).toHaveLength(1);
    expect(traffic.statusReads).toEqual([traffic.submissions[0].submission_id]);
    await expect(page.getByRole("textbox")).toBeDisabled();
    await expect(page.getByRole("button", { name: `Remove ${photo.name}`, exact: true })).toBeDisabled();
    traffic.admitLateOriginal();
    await retry.click();
    await expect(page.getByLabel("Temporary attachments")).toHaveCount(0);
    expect(traffic.submissions).toHaveLength(2);
    expect(traffic.submissions[1]).toEqual(traffic.submissions[0]);
    expect(traffic.submissions[1].attachment_ids).toEqual(traffic.uploads.map((upload) => upload.id));
    expect(traffic.acceptedKeys.size).toBe(1);
  });
  test("410 status preserves the draft and permits a new explicit intent with a fresh key", async ({ page }) => {
    const traffic = await install(page, { lostSubmission: "expired" });
    await page.getByLabel("Choose files").setInputFiles(photo);
    await expect(page.getByText("Temporary · ready")).toBeVisible();
    await page.getByRole("textbox").fill("Preserve my draft.");
    await page.getByRole("button", { name: "Send prompt", exact: true }).click();
    await page.getByRole("button", { name: "Check delivery status", exact: true }).click();
    await expect(page.getByRole("alert")).toContainText("Submission expired and cannot be delivered.");
    await expect(page.getByRole("textbox")).toBeEnabled();
    await expect(page.getByRole("textbox")).toHaveValue("Preserve my draft.");
    expect(traffic.submissions).toHaveLength(1);
    await expect(page.getByText("Temporary · ready")).toBeVisible();
    await page.getByRole("button", { name: "Send prompt", exact: true }).click();
    await expect(page.getByLabel("Temporary attachments")).toHaveCount(0);
    expect(traffic.submissions).toHaveLength(2);
    expect(traffic.submissions[1].submission_id).not.toBe(traffic.submissions[0].submission_id);
    expect(traffic.submissions[1].attachment_ids).toEqual(traffic.submissions[0].attachment_ids);
  });
});
