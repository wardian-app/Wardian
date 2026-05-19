#!/usr/bin/env node
import { execFileSync } from "node:child_process";

const MARKDOWN_IMAGE_PATTERN = /!\[[^\]]*]\(https:\/\/[^)\s]+\.(?:png|jpe?g|webp|gif)(?:[?#][^)\s]*)?\)/i;
const HTML_IMAGE_PATTERN = /<img\b[^>]*\bsrc=["']https:\/\/[^"']+\.(?:png|jpe?g|webp|gif)(?:[?#][^"']*)?["'][^>]*>/i;

const FRONTEND_PATTERNS = [
  /^src\/.+\.(css|ts|tsx)$/,
  /^public\//,
  /^index\.html$/,
  /^vite\.config\.ts$/,
  /^vitest\.config\.ts$/,
];

const NON_VISUAL_FRONTEND_PATTERNS = [
  /^src\/.+\.test\.(ts|tsx)$/,
];

function git(args) {
  return execFileSync("git", args, { encoding: "utf8" }).trim();
}

function changedFiles(base, head) {
  const output = git(["diff", "--name-status", "--diff-filter=ACMRT", `${base}...${head}`]);
  if (!output) return [];

  return output.split(/\r?\n/).flatMap((line) => {
    const fields = line.split(/\t+/).filter(Boolean);
    if (fields.length < 2) return [];
    return fields[0].startsWith("R") || fields[0].startsWith("C")
      ? [fields[fields.length - 1]]
      : [fields[1]];
  });
}

function isFrontendFile(file) {
  if (NON_VISUAL_FRONTEND_PATTERNS.some((pattern) => pattern.test(file))) {
    return false;
  }

  return FRONTEND_PATTERNS.some((pattern) => pattern.test(file));
}

function prBody() {
  return process.env.PR_BODY ?? "";
}

function hasEmbeddedScreenshot(body) {
  return MARKDOWN_IMAGE_PATTERN.test(body) || HTML_IMAGE_PATTERN.test(body);
}

const base = process.argv[2] ?? process.env.SCREENSHOT_BASE ?? "origin/main";
const head = process.argv[3] ?? process.env.SCREENSHOT_HEAD ?? "HEAD";
const files = changedFiles(base, head);
const frontendFiles = files.filter(isFrontendFile);

if (frontendFiles.length === 0) {
  console.log("No frontend changes detected; screenshot evidence is not required.");
  process.exit(0);
}

if (hasEmbeddedScreenshot(prBody())) {
  console.log("Frontend screenshot evidence found in the PR body.");
  process.exit(0);
}

console.error("Frontend changes require embedded screenshot evidence in the PR body.");
console.error("Changed frontend files:");
for (const file of frontendFiles) {
  console.error(`- ${file}`);
}
console.error("");
console.error("Attach or upload a feature-specific screenshot, then embed it with markdown such as:");
console.error("  ![queue-overflow.png](https://github.com/<owner>/<repo>/.../queue-overflow.png)");
process.exit(1);
