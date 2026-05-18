import { defineConfig } from "vitepress";

const base = process.env.DOCS_BASE ?? "/";

export default defineConfig({
  title: "Wardian",
  description:
    "Public documentation for Wardian, the local command center for multi-agent CLI workflows.",
  base,
  cleanUrls: true,
  lastUpdated: true,
  ignoreDeadLinks: [
    // Specs intentionally reference future and external planning artifacts.
    /ROADMAP\.md/,
  ],
  head: [
    ["meta", { name: "theme-color", content: "#2f6f6a" }],
    ["meta", { property: "og:type", content: "website" }],
    ["meta", { property: "og:site_name", content: "Wardian Docs" }],
  ],
  themeConfig: {
    siteTitle: "Wardian Docs",
    search: {
      provider: "local",
    },
    nav: [
      { text: "Guide", link: "/guide/" },
      { text: "Workflows", link: "/workflows/" },
      { text: "Providers", link: "/providers" },
      { text: "Developer", link: "/developer/" },
      {
        text: "GitHub",
        link: "https://github.com/tangemicioglu/Wardian",
      },
    ],
    sidebar: {
      "/guide/": [
        {
          text: "First-Time Setup",
          items: [
            { text: "Getting Started", link: "/guide/getting-started" },
            { text: "First-Run Troubleshooting", link: "/guide/first-run-troubleshooting" },
            { text: "UI Overview", link: "/guide/ui-overview" },
          ],
        },
        {
          text: "Agent Operations",
          items: [
            { text: "Wardian CLI", link: "/guide/cli" },
            { text: "Class Management", link: "/guide/class-management" },
            { text: "Library", link: "/guide/library" },
            { text: "Command Panel", link: "/guide/command-panel" },
            { text: "Watchlists", link: "/guide/watchlists" },
            { text: "Queue", link: "/guide/queue" },
            { text: "Explorer", link: "/guide/explorer" },
          ],
        },
        {
          text: "Source Control and Runtime",
          items: [
            { text: "Source Control", link: "/guide/source-control" },
            { text: "Settings", link: "/guide/settings" },
          ],
        },
        {
          text: "Workflows",
          items: [
            { text: "Workflow View", link: "/guide/workflows" },
            { text: "Workflow Reference", link: "/workflows/" },
          ],
        },
      ],
      "/workflows/": [
        {
          text: "Workflow Reference",
          items: [
            { text: "Overview", link: "/workflows/" },
            { text: "Building Workflows", link: "/workflows/building-workflows" },
            { text: "Agent Assignment", link: "/workflows/agent-assignment" },
            { text: "Triggers", link: "/workflows/triggers" },
            { text: "Scheduled Runs", link: "/workflows/scheduled-runs" },
            { text: "Node Reference", link: "/workflows/node-reference" },
            { text: "Troubleshooting", link: "/workflows/troubleshooting" },
          ],
        },
      ],
      "/developer/": [
        {
          text: "Developer Docs",
          items: [
            { text: "Overview", link: "/developer/" },
            { text: "Architecture", link: "/developer/architecture" },
            { text: "Setup", link: "/developer/setup" },
            { text: "State Management", link: "/developer/state-management" },
            { text: "IPC Events", link: "/developer/ipc-events" },
            { text: "Tauri Commands", link: "/developer/tauri-command-reference" },
            { text: "Provider Runtimes", link: "/developer/provider-runtimes" },
            { text: "PTY Lifecycle", link: "/developer/pty-lifecycle" },
            { text: "Native E2E", link: "/developer/native-e2e" },
            { text: "Theming", link: "/developer/theming" },
            { text: "Screenshot Documentation", link: "/developer/screenshot-documentation" },
          ],
        },
      ],
      "/": [
        {
          text: "Start Here",
          items: [
            { text: "Documentation Index", link: "/" },
            { text: "Getting Started", link: "/guide/getting-started" },
            { text: "Key Features", link: "/features" },
            { text: "Providers", link: "/providers" },
            { text: "OS Support", link: "/os-support" },
          ],
        },
      ],
    },
    socialLinks: [
      { icon: "github", link: "https://github.com/tangemicioglu/Wardian" },
    ],
    footer: {
      message: "Released under the MIT License.",
      copyright: "Copyright Tan Gemicioglu",
    },
  },
});
