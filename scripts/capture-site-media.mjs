/**
 * Capture the looping clips the Wardian feature site is built from.
 *
 * Records the real app against the same deterministic fixtures the
 * documentation stills use, transcodes each recording to mp4 and webm, emits a
 * poster frame, and writes `docs/assets/site-media/manifest.json` for the site
 * to consume.
 *
 * Unlike the stills, this deliberately does **not** apply `stabilizeVisuals()`.
 * That helper kills animation so screenshots are byte-stable; applying it here
 * would flatten every clip into a still image.
 */
import { chromium } from "@playwright/test";
import { spawnSync } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";

import {
  automations,
  installTauriDocsMock,
  libraryIndex,
  queueItems,
  workbenchDocument,
} from "./lib/docs-app-mock.mjs";
import {
  NAVIGATION_TIMEOUT_MS,
  resolveDevServerTarget,
  startOwnedServer,
  stopOwnedServer,
  waitForServer,
  warmUpDevServer,
} from "./lib/docs-dev-server.mjs";

const root = process.cwd();
const outputDir = path.join(root, "docs", "assets", "site-media");

/**
 * Recordings and transcodes are staged outside the repository.
 *
 * The dev server watches the project directory. Writing a clip's mp4 into
 * `docs/` mid-run triggers an HMR full reload, which reloads the page in the
 * middle of the *next* clip and quietly corrupts it. Staging in the OS temp
 * directory keeps every intermediate write away from the watcher; the finished
 * files are copied into `docs/assets/site-media/` once the server is stopped.
 */
let stageDir;

const serverTarget = resolveDevServerTarget({
  root,
  urlEnv: "WARDIAN_SITE_MEDIA_URL",
  portEnv: "WARDIAN_SITE_MEDIA_PORT",
  // A different default port from the stills capture, so the two can run
  // back to back without one refusing to start against the other's server.
  defaultPort: 1431,
  homeDirName: "wardian-site-media",
});
const baseUrl = serverTarget.baseUrl;

/**
 * Capture size, chosen from how large the clip actually renders.
 *
 * The site shows a clip at roughly 1100-1250 CSS pixels. A 1600px capture was
 * therefore displayed at 0.32-0.40x, which turns 12px interface labels into
 * four or five pixels and makes every clip unreadable. Recording near the
 * display width keeps interface text close to 1:1 instead of destroying it in
 * the downscale.
 */
const VIDEO_SIZE = { width: 1280, height: 800 };

/** Contract: every clip is 6-12 seconds and every mp4 is under 900 KB. */
const MIN_CLIP_MS = 6_000;
const MAX_CLIP_MS = 12_000;
const MAX_MP4_BYTES = 900 * 1024;

/** Let the app settle after boot before the recorded action starts. */
const SETTLE_MS = 2_500;

/**
 * Clips the site requires. A run that does not produce all of these fails.
 *
 * This mirrors `EXPECTED_CAPTURES` in the stills capture, and for the same
 * reason: the run is a long linear sequence, and a step that fails partway
 * silently abandons everything after it while leaving the previous run's files
 * on disk looking current. Failing loudly is not enough — the operator has to
 * be told which clips are now untrustworthy.
 */
const REQUIRED_CLIPS = [
  "hero",
  "graph",
  "inbox",
  "workflows",
  "dashboard",
  "garden",
  "markdown-truth",
  "classes",
];

/**
 * Clips that are nice to have. A stretch clip that cannot be driven is
 * reported and skipped rather than failing the run or shipping something empty.
 *
 * Empty today. `garden` was here while no section used it; now that the site
 * has a Garden section, a skipped capture would publish a section with no
 * media, so it is required.
 *
 * `terminal-mirrors` is deliberately absent from both lists. The
 * Owner/Mirror/Connecting badge only appears once two surfaces are bound to the
 * same agent session, and the Open Surface palette stops offering
 * `agent-session` once one is open — a second one needs a split-group duplicate
 * gesture this harness does not drive. The surface is real; filming it is not a
 * cheap extension, so the section goes without a clip rather than with an empty
 * one.
 */
const STRETCH_CLIPS = [];

const EXPECTED_CLIPS = [...REQUIRED_CLIPS, ...STRETCH_CLIPS];

const captured = new Map();

function missedRequired() {
  return REQUIRED_CLIPS.filter((id) => !captured.has(id));
}

function missedStretch() {
  return STRETCH_CLIPS.filter((id) => !captured.has(id));
}

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * First-run coach marks are dismissed for every clip.
 *
 * They are the right thing for a new install and the wrong thing on camera:
 * `graph-topology-actions:v1` renders directly over a node label in the Graph,
 * and a product filmed in tutorial mode reads as a product mid-setup. The
 * stills capture keeps its own list, so documentation can still show a hint
 * where a guide is explaining one.
 */
const CLIP_DISMISSED_HINT_IDS = [
  "spawn-agent-first-run:v1",
  "graph-topology-actions:v1",
  "automation-authoring:v1",
];

/**
 * The left configuration pane is collapsed for every clip.
 *
 * No section is about it, and open it costs roughly a fifth of the frame to a
 * form nobody is being asked to look at — which in a clip shown at around a
 * thousand pixels is a fifth of the legibility budget. Collapsed, the surface
 * under discussion relayouts to fill the width. The icon rail stays, so the
 * app still reads as itself.
 */
function withClipDefaults(mock) {
  const fixtures = { ...mock.fixtures };
  const shell = { ...workbenchDocument.shell, left_sidebar_collapsed: true };
  return {
    ...mock,
    fixtures: {
      dismissedOnboardingHintIds: CLIP_DISMISSED_HINT_IDS,
      ...fixtures,
      workbenchDocument: {
        ...workbenchDocument,
        ...(fixtures.workbenchDocument ?? {}),
        shell: { ...shell, ...(fixtures.workbenchDocument?.shell ?? {}) },
      },
    },
  };
}

// ---------------------------------------------------------------------------
// Fixtures layered on top of the shared documentation mock.
// ---------------------------------------------------------------------------

/** Graph needs a topology to draw. Three agents, two manual edges, one rule. */
const topology = {
  edges: [
    { a: "docs-codex", b: "docs-reviewer", origin: "manual" },
    { a: "docs-codex", b: "docs-designer", origin: "rule:same-workspace:1" },
  ],
  ignored_pairs: [],
  fallback_groups: [["docs-codex", "docs-reviewer", "docs-designer"]],
};

const pairActivity = {
  pairs: [
    {
      a: "docs-codex",
      b: "docs-reviewer",
      last_message_at: "2026-05-12T10:18:00.000Z",
      active_ask: true,
      awaiting_reply_from: "docs-reviewer",
    },
    {
      a: "docs-codex",
      b: "docs-designer",
      last_message_at: "2026-05-12T09:52:00.000Z",
      active_ask: false,
      awaiting_reply_from: null,
    },
  ],
  truncated: false,
  next_offset: null,
};

/**
 * Inbox items for the ask/reply clip.
 *
 * The point of the section is that a handoff between agents leaves a record:
 * an ask that is still awaiting a reply, and the same exchange once answered.
 */
const inboxQueueItems = [
  {
    id: "docs-ask-pending",
    type: "action_needed",
    timestamp: 1778590740000,
    read: false,
    agent_session_id: "docs-reviewer",
    agent_name: "Docs-Reviewer",
    inbox_notification_id: "ask-1",
    notification_status: "awaiting_reply",
    notification_title: "Docs-Codex asked Docs-Reviewer for a decision",
    proposed_action:
      "Replace the hero capture with the regenerated grid screenshot before tagging the release.",
    risk: "Low. The previous asset stays in history and the change is reversible.",
    approval_choices: ["Approve", "Reject", "Ask for changes"],
    summary:
      "Docs-Codex finished the capture pass and needs a decision before the asset is swapped.\n\nThe ask carries the proposed action, the risk assessment, and the choices the replying agent may return. Nothing is applied until a reply lands.",
  },
  {
    id: "docs-ask-answered",
    type: "agent_completed",
    timestamp: 1778590500000,
    read: true,
    agent_session_id: "docs-codex",
    agent_name: "Docs-Codex",
    inbox_notification_id: "ask-0",
    notification_status: "completed",
    notification_title: "Docs-Reviewer replied to Docs-Codex",
    summary:
      "Reply recorded: the regenerated Explorer capture is accepted, no blocking findings.\n\nThe exchange stays in the Inbox as a durable record of who asked, what was proposed, and what came back.",
  },
  ...queueItems,
];

/** A blueprint that actually branches, loops and waits. */
const controlFlowAutomation = {
  id: "docs-release-flow",
  name: "Release Media Refresh",
  settings: { max_iterations: 5, on_limit_reached: "pause" },
  nodes: [
    {
      id: "trigger-1",
      type: "manual_trigger",
      name: "Manual Trigger",
      config: { type: "manual" },
      position: { x: 80, y: 220 },
    },
    {
      id: "task-1",
      type: "task",
      name: "Capture clips",
      config: {
        agent_class: "Coder",
        prompt: "Capture the site media clips and report the manifest.",
      },
      dependencies: [{ node_id: "trigger-1", port: "default" }],
      position: { x: 320, y: 220 },
    },
    {
      id: "branch-1",
      type: "branch",
      name: "Under size budget?",
      config: { condition: "clip.bytes_mp4 < 900000" },
      dependencies: [{ node_id: "task-1", port: "default" }],
      position: { x: 580, y: 220 },
    },
    {
      id: "loop-1",
      type: "loop",
      name: "Shorten and retry",
      config: { over: "oversized_clips", max_iterations: 3 },
      dependencies: [{ node_id: "branch-1", port: "false" }],
      position: { x: 840, y: 360 },
    },
    {
      id: "approval-1",
      type: "approval",
      name: "Approve the refresh",
      config: { prompt: "Publish the regenerated clips?" },
      dependencies: [{ node_id: "branch-1", port: "true" }],
      position: { x: 840, y: 100 },
    },
    {
      id: "join-1",
      type: "join",
      name: "Join",
      config: {},
      dependencies: [
        { node_id: "approval-1", port: "default" },
        { node_id: "loop-1", port: "default" },
      ],
      position: { x: 1100, y: 220 },
    },
  ],
};

/** Library index with a populated Classes section for the `classes` clip. */
const classesLibraryIndex = {
  ...libraryIndex,
  sections: {
    ...libraryIndex.sections,
    classes: {
      stubbed: false,
      tree: {
        path: "",
        name: "Root",
        children: [
          {
            kind: "class",
            name: "Reviewer",
            path: "reviewer.md",
            entry_ref: "classes/reviewer.md",
            description: "Patch review and risk analysis. Findings before prose.",
            tags: ["review"],
            is_starred: true,
            deployment_count: 2,
            error: null,
          },
          {
            kind: "class",
            name: "Coder",
            path: "coder.md",
            entry_ref: "classes/coder.md",
            description: "Implementation and verification work.",
            tags: ["build"],
            is_starred: false,
            deployment_count: 3,
            error: null,
          },
        ],
      },
    },
  },
};

/**
 * A Library with something in every section, for the Markdown-as-Truth clip.
 *
 * The section names four kinds of file. The default fixture holds one prompt
 * and nothing else, so the rail showed four empty sections next to a sentence
 * claiming all four are files — the claim and the screen disagreed.
 */
const populatedLibraryIndex = {
  ...libraryIndex,
  sections: {
    ...libraryIndex.sections,
    skills: {
      stubbed: false,
      tree: {
        path: "",
        name: "Root",
        children: [
          {
            kind: "skill",
            name: "Root Cause Analysis",
            path: "root-cause-analysis/SKILL.md",
            entry_ref: "skills/root-cause-analysis/SKILL.md",
            description: "Trace a failure to the change that caused it.",
            tags: ["method"],
            is_starred: false,
            deployment_count: 3,
            error: null,
          },
          {
            kind: "skill",
            name: "Pull Request",
            path: "pull-request/SKILL.md",
            entry_ref: "skills/pull-request/SKILL.md",
            description: "Conventions and checks for opening a PR.",
            tags: ["artifact"],
            is_starred: true,
            deployment_count: 5,
            error: null,
          },
        ],
      },
    },
    prompts: {
      stubbed: false,
      tree: {
        path: "",
        name: "Root",
        children: [
          ...libraryIndex.sections.prompts.tree.children,
          {
            kind: "prompt",
            name: "Automation Plan",
            path: "automation/plan.md",
            entry_ref: "prompts/automation/plan.md",
            description: "Break a task into bounded agent steps.",
            tags: ["automation"],
            is_starred: false,
            deployment_count: 0,
            error: null,
          },
        ],
      },
    },
    classes: classesLibraryIndex.sections.classes,
    automations: {
      stubbed: false,
      tree: {
        path: "",
        name: "Root",
        children: [
          {
            kind: "automation",
            name: "Docs Screenshot Refresh",
            path: "docs-screenshot-refresh.json",
            entry_ref: "automations/docs-screenshot-refresh.json",
            description: "Recapture the guide screenshots and verify them.",
            tags: ["docs"],
            is_starred: false,
            deployment_count: 1,
            error: null,
          },
        ],
      },
    },
  },
};

// ---------------------------------------------------------------------------
// Page helpers.
// ---------------------------------------------------------------------------

async function openSurface(page, surfaceType) {
  await page.keyboard.press(process.platform === "darwin" ? "Meta+P" : "Control+P");
  const dialog = page.getByRole("dialog", { name: "Open Surface" });
  await dialog.waitFor({ timeout: 15_000 });
  await dialog.locator(`[role="option"][data-surface-type="${surfaceType}"]`).click();
  await page
    .locator(`[role="tab"][data-surface-type="${surfaceType}"][aria-selected="true"]`)
    .waitFor({ timeout: 15_000 });
}

/** Selection lives on the card header; a click on the card body selects nothing. */
async function selectAgent(page, sessionId) {
  await page.locator(`[data-testid="agent-card-header-${sessionId}"]`).click();
}

async function pushPty(page, sessionId, chunk) {
  await page.evaluate(
    ([id, text]) => window.__WARDIAN_DOCS_PUSH_PTY?.(id, text),
    [sessionId, chunk],
  );
}

/**
 * Strings that look like a real path but are not.
 *
 * `C:/projects/my-app` is the spawn form's own placeholder text, shipped in the
 * app, and `<absolute-workspace-path>` is the fixture placeholder.
 */
const ALLOWED_PATH_LIKE = ["C:/projects/my-app", "<absolute-workspace-path>"];

/**
 * Fail if anything path-shaped reached the screen.
 *
 * The repository and the site are both public, so a clip must never carry a
 * real workspace path, home directory, or username. Checking the rendered text
 * is stronger than reviewing frames by eye and it runs on every capture, so a
 * fixture that starts leaking later cannot ship quietly.
 */
async function assertNoLeakedPaths(page, clipId, stage) {
  const text = await page.evaluate(() => document.body.innerText);
  let scrubbed = text;
  for (const allowed of ALLOWED_PATH_LIKE) scrubbed = scrubbed.split(allowed).join("");

  const offenders = [
    [/[A-Za-z]:[\\/][^\s"']*/g, "drive-letter path"],
    [/\/(?:home|Users)\/[^\s"']+/g, "home directory path"],
  ].flatMap(([pattern, label]) =>
    (scrubbed.match(pattern) ?? []).map((hit) => `${label}: ${hit}`),
  );

  // The capture's own checkout must never appear, whatever shape it takes.
  for (const form of [root, root.replace(/\\/g, "/")]) {
    if (scrubbed.includes(form)) offenders.push("repository path on screen");
  }

  if (offenders.length > 0) {
    throw new Error(
      `${clipId} (${stage}) would leak a path into a public clip: ${[...new Set(offenders)].join("; ")}`,
    );
  }
}

/** Type a line into a terminal a character group at a time, so it reads as live. */
async function typeIntoTerminal(page, sessionId, line, stepMs = 45) {
  for (const chunk of line.match(/.{1,3}/gs) ?? []) {
    await pushPty(page, sessionId, chunk);
    await wait(stepMs);
  }
}

// ---------------------------------------------------------------------------
// Clip choreography.
// ---------------------------------------------------------------------------

const CLIPS = [
  {
    id: "hero",
    mock: {},
    // The hero opens mid-keystroke, so its first frame reads as a typo. Point
    // the poster at the end state instead: both terminals settled, the ask
    // sent and the reply back.
    posterAtMs: 8_600,
    async prepare(page) {
      // Only the two running agents have grid cards. `docs-designer` is off,
      // so it appears in the roster and nowhere else.
      await page.locator('[data-testid="agent-grid"]').waitFor({ timeout: 15_000 });
    },
    async run(page) {
      await typeIntoTerminal(
        page,
        "docs-codex",
        "\r\n$ wardian ask Docs-Reviewer \"check the capture manifest\"\r\n",
      );
      await wait(700);
      await selectAgent(page, "docs-codex");
      await wait(1_200);
      await typeIntoTerminal(page, "docs-reviewer", "\r\n$ Reviewing the capture manifest...\r\n");
      await wait(700);
      await selectAgent(page, "docs-reviewer");
      await wait(1_300);
      await pushPty(page, "docs-reviewer", "No blocking findings.\r\n");
      await wait(1_500);
      await pushPty(page, "docs-codex", "Reply received. Manifest accepted.\r\n");
      await wait(1_800);
    },
  },
  {
    id: "graph",
    mock: {
      commandResults: { get_topology: topology, get_pair_activity: pairActivity },
    },
    async prepare(page) {
      await openSurface(page, "graph");
      await wait(2_500);
    },
    async run(page) {
      const canvas = page.locator('[data-testid="surface-panel"][data-surface-type="graph"]');
      await canvas.hover();
      await wait(1_600);
      await page.mouse.wheel(0, -160);
      await wait(2_200);
      await page.mouse.wheel(0, 120);
      await wait(2_000);
      await canvas.hover({ position: { x: 700, y: 420 } });
      await wait(1_800);
    },
  },
  {
    id: "inbox",
    mock: { fixtures: { queueItems: inboxQueueItems } },
    async prepare(page) {
      await openSurface(page, "inbox");
      await wait(1_800);
    },
    async run(page) {
      await wait(1_500);
      const expand = page.getByRole("button", { name: "Show full summary" }).first();
      if (await expand.isVisible().catch(() => false)) {
        await expand.click();
      }
      await wait(3_000);
      await page.mouse.wheel(0, 220);
      await wait(2_800);
    },
  },
  {
    id: "workflows",
    mock: {
      fixtures: { automations: [controlFlowAutomation, ...automations] },
    },
    async prepare(page) {
      await openSurface(page, "automations");
      await page.getByTestId("automations-view").waitFor({ timeout: 15_000 });
      await wait(2_600);
    },
    async run(page) {
      const canvas = page.getByTestId("automations-view");
      await canvas.hover();
      await wait(2_000);
      await page.mouse.wheel(0, -120);
      await wait(2_400);
      await page.mouse.wheel(0, 100);
      await wait(2_200);
      await canvas.hover({ position: { x: 780, y: 380 } });
      await wait(1_800);
    },
  },
  {
    id: "dashboard",
    mock: {},
    async prepare(page) {
      await openSurface(page, "dashboard");
      await page
        .locator('[data-testid="surface-panel"][data-surface-type="dashboard"] .dashboard-view__table')
        .waitFor({ timeout: 15_000 });
      await wait(1_200);
    },
    async run(page) {
      await wait(3_200);
      await openSurface(page, "analytics");
      await page
        .locator('[data-testid="surface-panel"][data-surface-type="analytics"] .analytics-view__matrix')
        .waitFor({ timeout: 15_000 });
      await wait(3_600);
    },
  },
  {
    id: "markdown-truth",
    mock: { fixtures: { libraryIndex: populatedLibraryIndex } },
    // The section claims the Library and the file tree are two views of one
    // directory, so the clip has to end on the same file it started from.
    // Deliberately no agent selection: the Explorer roots at an agent's
    // workspace when one is selected and at the Wardian home when none is, and
    // only the second root contains the Library's own files.
    //
    // The poster is the frame where both halves are on screen at once, not the
    // opening one — a still of the Library alone shows none of the claim.
    posterAtMs: 8_600,
    async prepare(page) {
      await openSurface(page, "library");
      await page.getByTestId("library-section-prompts").click();
      await wait(1_000);
    },
    async run(page) {
      await page.getByTestId("library-row-prompts/review/checklist.md").click();
      await wait(2_800);
      await page.locator('[data-testid="sidebar-tab-explorer"]').click();
      await wait(1_200);
      const explorer = page.locator('[data-testid="explorer-panel"]');
      for (const folder of ["library", "prompts", "review"]) {
        await explorer.getByText(folder, { exact: true }).click();
        await wait(900);
      }
      // Selecting the row highlights it, which is where the eye should land.
      // Opening it would need the whole Files resource protocol, and the point
      // is already made by the name sitting in the tree.
      await explorer.getByText("checklist.md", { exact: true }).click();
      await wait(2_000);
    },
  },
  {
    id: "classes",
    mock: { fixtures: { libraryIndex: classesLibraryIndex } },
    async prepare(page) {
      await openSurface(page, "library");
      await wait(1_200);
      await page.getByTestId("library-section-classes").click();
      await wait(1_600);
    },
    async run(page) {
      await wait(1_200);
      await page.getByTestId("library-row-classes/reviewer.md").click();
      await wait(3_400);
      await page.getByTestId("library-row-classes/coder.md").click();
      await wait(3_000);
    },
  },
  {
    id: "garden",
    mock: {},
    async prepare(page) {
      await openSurface(page, "garden");
      await wait(3_500);
    },
    async run(page) {
      const canvas = page.locator('[data-testid="surface-panel"][data-surface-type="garden"]');
      await canvas.hover();
      await wait(2_500);
      await page.mouse.wheel(0, -120);
      await wait(2_500);
      await canvas.hover({ position: { x: 720, y: 420 } });
      await wait(2_000);
    },
  },
];

// ---------------------------------------------------------------------------
// ffmpeg.
// ---------------------------------------------------------------------------

function runFfmpeg(args, label) {
  const result = spawnSync("ffmpeg", args, { encoding: "utf8" });
  if (result.error || result.status !== 0) {
    throw new Error(
      `ffmpeg failed for ${label}:\n${result.stderr?.slice(-2000) ?? result.error?.message}`,
    );
  }
}

function assertFfmpegAvailable() {
  for (const binary of ["ffmpeg", "ffprobe"]) {
    const probe = spawnSync(binary, ["-version"], { encoding: "utf8" });
    if (probe.error || probe.status !== 0) {
      throw new Error(
        `${binary} was not found on PATH. Site media capture transcodes Playwright's ` +
          `recording to mp4 and webm and cannot run without it. Install ffmpeg ` +
          `(https://ffmpeg.org/download.html) and re-run \`npm run site:media\`.`,
      );
    }
  }
}

function probeDurationMs(file) {
  const result = spawnSync(
    "ffprobe",
    ["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", file],
    { encoding: "utf8" },
  );
  if (result.status !== 0) throw new Error(`ffprobe failed for ${file}`);
  return Math.round(Number.parseFloat(result.stdout.trim()) * 1000);
}

/**
 * Transcode one recording into the three assets the site consumes.
 *
 * `startOffsetMs` drops the app boot that Playwright records before the
 * choreography begins — recording starts with the context, not with the action.
 */
function transcode(id, rawPath, startOffsetMs, posterAtMs = 0) {
  const seek = (startOffsetMs / 1000).toFixed(3);
  // The poster is the still a viewer stares at until the clip loads, and on a
  // reduced-motion or no-JS render it is the only thing they ever see. The
  // first frame is often mid-keystroke, so a clip may nominate a later one.
  const posterSeek = ((startOffsetMs + posterAtMs) / 1000).toFixed(3);
  const mp4 = path.join(stageDir, `${id}.mp4`);
  const webm = path.join(stageDir, `${id}.webm`);
  const poster = path.join(stageDir, `${id}.png`);
  const chain = `scale=${VIDEO_SIZE.width}:${VIDEO_SIZE.height}:flags=lanczos`;

  runFfmpeg(
    ["-y", "-ss", seek, "-i", rawPath, "-an", "-c:v", "libx264", "-profile:v", "high",
      // CRF 30 was chosen before anyone looked at a frame. Interface text is
      // high-frequency detail and is the first thing a high CRF destroys, and
      // the clips were using a tenth of the size budget, so there was no
      // reason to be economical.
      "-pix_fmt", "yuv420p", "-crf", "21", "-preset", "slower", "-movflags", "+faststart",
      "-vf", `fps=24,${chain}`, mp4],
    `${id}.mp4`,
  );
  runFfmpeg(
    ["-y", "-ss", seek, "-i", rawPath, "-an", "-c:v", "libvpx-vp9", "-crf", "30", "-b:v", "0",
      "-row-mt", "1", "-vf", `fps=24,${chain}`, webm],
    `${id}.webm`,
  );
  runFfmpeg(
    ["-y", "-ss", posterSeek, "-i", rawPath, "-frames:v", "1", "-vf", chain, poster],
    `${id}.png`,
  );

  return { mp4, webm, poster, width: VIDEO_SIZE.width, height: VIDEO_SIZE.height };
}

// ---------------------------------------------------------------------------
// Run.
// ---------------------------------------------------------------------------

async function recordClip(browser, clip) {
  const context = await browser.newContext({
    viewport: VIDEO_SIZE,
    deviceScaleFactor: 1,
    recordVideo: { dir: path.join(stageDir, "raw"), size: VIDEO_SIZE },
  });
  const startedAt = Date.now();
  const page = await context.newPage();

  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));

  let rawPath;
  let startOffsetMs;
  try {
    await installTauriDocsMock(page, withClipDefaults(clip.mock ?? {}));
    await page.goto(baseUrl, { waitUntil: "domcontentloaded", timeout: NAVIGATION_TIMEOUT_MS });
    await page.locator('[data-testid="app-shell"]').waitFor({ timeout: NAVIGATION_TIMEOUT_MS });
    await wait(SETTLE_MS);

    // Navigation to the clip's own surface happens before the recorded segment
    // starts, so the poster frame shows that surface rather than whatever the
    // app happens to boot into, and the clip's seconds are spent on the thing
    // the section is about.
    if (clip.prepare) await clip.prepare(page);

    if (await page.getByText("Fatal UI Rendering Error").isVisible().catch(() => false)) {
      throw new Error(`app is showing the error boundary: ${pageErrors.join("; ")}`);
    }

    await assertNoLeakedPaths(page, clip.id, "first frame");

    startOffsetMs = Date.now() - startedAt;
    await clip.run(page);

    if (await page.getByText("Fatal UI Rendering Error").isVisible().catch(() => false)) {
      throw new Error(`app crashed during the clip: ${pageErrors.join("; ")}`);
    }

    await assertNoLeakedPaths(page, clip.id, "last frame");
  } finally {
    const video = page.video();
    await page.close().catch(() => {});
    await context.close().catch(() => {});
    rawPath = video ? await video.path() : null;
  }

  if (!rawPath) throw new Error(`no recording was produced for ${clip.id}`);

  const rawDurationMs = probeDurationMs(rawPath);
  const durationMs = rawDurationMs - startOffsetMs;
  if (durationMs < MIN_CLIP_MS || durationMs > MAX_CLIP_MS) {
    throw new Error(
      `${clip.id} is ${(durationMs / 1000).toFixed(1)}s, outside the ${MIN_CLIP_MS / 1000}-${MAX_CLIP_MS / 1000}s budget. Adjust its choreography.`,
    );
  }

  const files = transcode(clip.id, rawPath, startOffsetMs, clip.posterAtMs ?? 0);
  const bytesMp4 = (await fs.stat(files.mp4)).size;
  if (bytesMp4 > MAX_MP4_BYTES) {
    throw new Error(
      `${clip.id}.mp4 is ${(bytesMp4 / 1024).toFixed(0)} KB, over the ${MAX_MP4_BYTES / 1024} KB budget. Shorten the clip rather than shipping it oversized.`,
    );
  }

  return {
    id: clip.id,
    mp4: `${clip.id}.mp4`,
    webm: `${clip.id}.webm`,
    poster: `${clip.id}.png`,
    width: files.width,
    height: files.height,
    duration_ms: durationMs,
    bytes_mp4: bytesMp4,
  };
}

async function main() {
  assertFfmpegAvailable();

  for (const clip of CLIPS) {
    if (!EXPECTED_CLIPS.includes(clip.id)) {
      // Keeps the manifest honest: a clip added to the sequence and not to the
      // list would never be reported as skipped.
      throw new Error(`${clip.id} is captured but missing from EXPECTED_CLIPS`);
    }
  }

  stageDir = await fs.mkdtemp(path.join(os.tmpdir(), "wardian-site-media-"));
  await fs.mkdir(path.join(stageDir, "raw"), { recursive: true });
  await fs.mkdir(serverTarget.home, { recursive: true });

  let server = null;
  if (!serverTarget.explicitBaseUrl) {
    server = await startOwnedServer(serverTarget, root);
  }
  await waitForServer(baseUrl);

  const failures = [];
  const browser = await chromium.launch();
  try {
    await warmUpDevServer(browser, {
      baseUrl,
      installMock: (page) => installTauriDocsMock(page),
    });

    for (const clip of CLIPS) {
      try {
        const entry = await recordClip(browser, clip);
        captured.set(clip.id, entry);
        console.log(
          `captured ${clip.id} (${(entry.duration_ms / 1000).toFixed(1)}s, ${(entry.bytes_mp4 / 1024).toFixed(0)} KB)`,
        );
      } catch (error) {
        if (clip.stretch) {
          console.warn(`skipped stretch clip ${clip.id}: ${error.message}`);
          continue;
        }
        // Carry on rather than stopping at the first failure. One broken clip
        // should not hide the state of the other seven — a run that reports
        // every fault at once is the difference between one fix per run and
        // one fix per attempt.
        failures.push(`${clip.id}: ${error.message}`);
        console.error(`FAILED ${clip.id}: ${error.message}`);
      }
    }
  } finally {
    await browser.close();
    if (server) stopOwnedServer(server);
  }

  if (failures.length > 0) {
    throw new Error(
      `${failures.length} required clip(s) failed:\n${failures.map((line) => `  ${line}`).join("\n")}`,
    );
  }

  const manifest = {
    generated_at: new Date().toISOString(),
    app_version: JSON.parse(await fs.readFile(path.join(root, "package.json"), "utf8")).version,
    clips: EXPECTED_CLIPS.filter((id) => captured.has(id)).map((id) => captured.get(id)),
  };

  // Publish only now that the dev server is stopped, and replace the directory
  // wholesale so a clip dropped from the manifest cannot linger on disk.
  await fs.rm(outputDir, { recursive: true, force: true });
  await fs.mkdir(outputDir, { recursive: true });
  for (const clip of manifest.clips) {
    for (const file of [clip.mp4, clip.webm, clip.poster]) {
      await fs.copyFile(path.join(stageDir, file), path.join(outputDir, file));
    }
  }
  await fs.writeFile(
    path.join(outputDir, "manifest.json"),
    `${JSON.stringify(manifest, null, 2)}\n`,
  );
  await fs.rm(stageDir, { recursive: true, force: true });
  console.log(
    `wrote ${manifest.clips.length} clip(s) to ${path.relative(root, outputDir).replace(/\\/g, "/")}`,
  );
}

/** One clip id per line, indented, for a console list. */
function listClips(ids) {
  return ids.map((id) => `  ${id}`).join("\n");
}

main()
  .then(() => {
    const missed = missedRequired();
    if (missed.length > 0) {
      console.error(
        `\nRun finished but ${missed.length} required clip(s) never ran:\n${listClips(missed)}`,
      );
      process.exit(1);
    }
    const skipped = missedStretch();
    if (skipped.length > 0) {
      console.warn(`\nStretch clip(s) not produced:\n${listClips(skipped)}`);
    }
  })
  .catch((error) => {
    console.error(error);
    const missed = missedRequired();
    if (missed.length > 0) {
      // The point of naming them: any files already on disk from an earlier run
      // still look current and are not.
      console.error(
        `\n${missed.length} required clip(s) were never reached and are now stale on disk:\n${listClips(missed)}`,
      );
    }
    process.exit(1);
  });
