# Agent Runtime Sandbox References

This document maps public agent runtime, sandbox, browser automation, and policy systems to design patterns relevant to Wardian's provider execution model, terminal safety, workflow approvals, and future isolated workspaces.

This is not an endorsement, affiliation claim, product evaluation, or competitive teardown. The notes below describe public architecture and design pressure only.

Last reviewed: 2026-05-12.

Source basis: entries were selected through public research and checked against public project pages, repositories, or documentation where available.

## Design Axes

- **Execution boundary**: whether agents run directly on the host, in a PTY, in a worktree, in a container, in a VM, in a browser session, or in a remote sandbox.
- **Policy model**: how filesystem, network, secrets, shell, browser, and desktop permissions are granted or denied.
- **Persistence model**: whether sandbox state is disposable, resumable, snapshot-based, or long-running.
- **Human intervention**: whether users can inspect, attach, approve, pause, or terminate execution.
- **Agent API**: whether sandbox lifecycle can be managed through CLI, REST, MCP, SDK, or UI.
- **Audit trail**: whether commands, browser actions, file changes, credentials, and network access are logged.

## Summary Map

| System | Primary Pattern | Wardian Takeaway |
|---|---|---|
| [NVIDIA OpenShell](https://github.com/NVIDIA/OpenShell) | Private agent runtime with declarative YAML policies. | Strong reference for permissioned local execution and data-exfiltration controls. |
| [Agent-Sandbox](https://github.com/agent-sandbox/agent-sandbox) | Kubernetes-backed multi-tenant agent sandbox with REST and MCP lifecycle management. | Useful for future remote/isolated Wardian workers. |
| [E2B](https://e2b.dev/) | Cloud sandbox for AI-generated code and agents. | Reference for disposable execution environments and agent-accessible terminals. |
| [Daytona](https://github.com/daytonaio/daytona) | Development-environment sandbox infrastructure. | Relevant to isolated workspaces and repeatable dev environments. |
| [Stagehand](https://github.com/browserbase/stagehand) | AI browser automation primitives over Chromium. | Useful if Wardian adds browser panels or browser workflow nodes. |
| [Browserbase MCP](https://github.com/browserbase/mcp-server-browserbase) | MCP access to managed browser sessions. | Reference for browser sessions as agent tools with replay and lifecycle control. |
| [browser-use](https://github.com/browser-use/browser-use) | Python browser automation for agents. | Useful for comparing free-form browser agent loops against deterministic primitives. |
| [Playwright MCP](https://github.com/microsoft/playwright-mcp) | Browser automation exposed through MCP. | Relevant for local, inspectable browser tooling in workflows. |
| [Skyvern](https://github.com/Skyvern-AI/skyvern) | AI browser automation for web workflows. | Useful as an adjacent workflow/browser automation reference. |

## Reference Profiles

### NVIDIA OpenShell

**Source basis:** Public repo checked.

**What it includes:** OpenShell is a safe private runtime for autonomous AI agents. It provides sandboxed execution environments governed by declarative YAML policies intended to prevent unauthorized file access, data exfiltration, and uncontrolled network activity.

**Distinctive components:**

- Sandboxed execution for coding agents such as Claude, OpenCode, Codex, and Copilot.
- Declarative policy files.
- File access controls.
- Network and exfiltration-oriented governance.
- Terminal UI for debugging.

**Wardian relevance:** OpenShell is the most directly relevant policy reference. Wardian should eventually represent what an agent may read, write, execute, and exfiltrate as explicit policy, not only as user trust in a provider.

### Agent-Sandbox

**Source basis:** Public repo checked.

**What it includes:** Agent-Sandbox is an AI-first sandbox system built around Kubernetes. It supports code execution, browser automation, computer use, shell commands, multi-session and multi-tenant isolation, REST APIs, MCP lifecycle management, storage, logs, terminal access, and UI surfaces.

**Distinctive components:**

- Sandbox lifecycle API.
- MCP server for agents to create/access/delete sandboxes.
- Code, browser, computer, and shell runtime categories.
- Multi-session and multi-tenant isolation.
- Unified storage across sandbox types.
- Kubernetes-backed deployment.

**Wardian relevance:** This is more cloud-native than Wardian's near-term local desktop runtime, but its lifecycle model matters. Wardian should model execution environments explicitly: host PTY, worktree, container, remote sandbox, browser session, and future VM should be different environment types with different guarantees.

### E2B

**Source basis:** Public documentation checked.

**What it includes:** E2B provides secure cloud environments for running AI-generated code and agents. It is commonly used as a sandbox runtime by agent systems such as OpenHands.

**Distinctive components:**

- Isolated cloud sandboxes.
- Programmatic SDKs and CLI access.
- Terminal connection to running sandboxes.
- File and command execution APIs.
- Agent-runtime orientation.

**Wardian relevance:** E2B is useful as a reference for disposable and remote execution. Wardian should not require cloud sandboxes, but workflow nodes should be able to declare whether they run on the host, in a worktree, or in an isolated runtime.

### Daytona

**Source basis:** Public repo checked at a high level.

**What it includes:** Daytona provides development-environment infrastructure for creating and managing isolated dev environments. It is relevant less as an agent system and more as a repeatable workspace/runtime abstraction.

**Distinctive components:**

- Environment provisioning.
- Workspace lifecycle management.
- Developer-tool integration.
- Remote and reproducible development environments.

**Wardian relevance:** Wardian's worktree model may eventually need richer environment provisioning: dependencies, services, ports, secrets, and remote machines. Daytona belongs in the long-term environment-management reference set.

### Stagehand

**Source basis:** Public site checked.

**What it includes:** Stagehand is an open-source AI browser automation framework with primitives such as `act`, `extract`, `observe`, and `agent`. It can run locally with Chromium and optionally use Browserbase for managed sessions, replay, captcha handling, and identity.

**Distinctive components:**

- Natural-language browser automation primitives.
- Deterministic step-by-step control with an agent option for multi-step flows.
- Local Chromium support.
- Optional managed Browserbase sessions.
- Support across major model providers.

**Wardian relevance:** Stagehand is relevant if Wardian adds browser workflow nodes or browser panels. Its split between deterministic primitives and autonomous agent mode maps well to Wardian's distinction between workflow control and agent autonomy.

### Browserbase MCP

**Source basis:** Public changelog/repo checked.

**What it includes:** Browserbase exposes browser automation through an MCP server powered by Stagehand and managed browser infrastructure.

**Distinctive components:**

- MCP interface for browser actions.
- Managed cloud browser sessions.
- Session persistence and replay.
- Production browser automation infrastructure.

**Wardian relevance:** Browserbase MCP shows how browser sessions can become structured tools. Wardian should be able to attach a browser surface to a workflow run while preserving replayable action logs and human inspection.

### browser-use

**Source basis:** Public repo checked at a high level.

**What it includes:** browser-use is a Python framework for letting agents control browsers, typically through Playwright-style automation and model-driven loops.

**Distinctive components:**

- Browser control for agents.
- Model-agnostic automation loops.
- Python developer ergonomics.
- Broad community adoption.

**Wardian relevance:** browser-use is useful as a caution and comparison point. Fully autonomous browser loops are powerful, but Wardian should prefer structured action logs, selectors, permissions, and replay where possible.

### Playwright MCP

**Source basis:** Public repo checked at a high level.

**What it includes:** Playwright MCP exposes browser automation capabilities through MCP, allowing agents to operate browsers through a structured tool interface.

**Distinctive components:**

- Local browser automation through Playwright.
- MCP tool interface.
- Structured browser actions.
- Screenshot/DOM-driven inspection patterns.

**Wardian relevance:** This is likely the most practical browser automation reference for local-first Wardian workflows. A browser node should be observable, replayable, and interruptible, not just a hidden agent tool call.

### Skyvern

**Source basis:** Public repo checked at a high level.

**What it includes:** Skyvern is an AI browser automation system for web workflows. It is relevant as part of the broader browser-agent automation landscape.

**Distinctive components:**

- Browser workflow automation.
- AI-assisted interaction with web pages.
- Task execution and workflow posture.

**Wardian relevance:** Skyvern is a later-stage reference. Wardian does not need to become a browser automation product, but browser workflows will matter if agents need to operate web apps during visible runs.

## Wardian Positioning

Wardian should model runtime as a first-class part of every agent and workflow node:

```text
task -> runtime environment -> permissions -> observable session -> artifacts and audit log
```

Near-term implications:

- Represent host PTY, worktree, browser, container, and remote sandbox as distinct execution environments.
- Add policy metadata before adding powerful automation.
- Keep secrets and network access explicit.
- Make sandbox attach/inspect/terminate operations visible in the command center.
- Log runtime actions as workflow evidence, not just terminal text.
- Prefer deterministic browser/tool primitives for critical workflow steps, with autonomous agents reserved for exploration.
