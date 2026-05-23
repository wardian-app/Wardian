# Antigravity Clear Session Fix

- **Status:** Implemented
- **Date:** 2026-05-23
- **Decider:** Antigravity and Wardian maintainers

## Context

For `antigravity` agents, clearing the session using Wardian's clear command resets the internal config state (specifically setting `resume_session = None`), but doesn't prevent the background watcher thread from immediately falling back to and resuming the prior session. 

When a session is cleared, `spawn_agent` is called with `is_restored = false` (which maps to `watcher_skip_existing_log = false` in the watcher thread). Since `config.resume_session` is `None`, the watcher thread immediately auto-detects the last active session ID from the workspace cache (`last_conversations.json`) or the newest conversation ID under `brain/`. It writes this prior conversation ID back to `config.resume_session`, resuming it and polling its log path, even though the spawned `agy` process is running a brand new conversation.

## Decision

To allow `antigravity` agents to be fully cleared and reset:

1. In the watcher thread logic inside `src-tauri/src/manager/spawn.rs`, when the thread is started with `is_restored = false` (fresh launch or cleared launch), capture the conversation ID currently stored in the workspace cache or `brain/` directories as the `initial_conv_id`.
2. Inside the watcher loop, when detecting the conversation ID to monitor, if the detected ID matches `initial_conv_id`, treat it as `None` (ignore it).
3. Once the freshly spawned `agy` process writes a brand new conversation ID (which will not match `initial_conv_id`), the watcher thread will detect and lock onto it.

## Consequences

- Antigravity agents can be successfully cleared. The prior session will not be auto-resumed when starting a fresh session.
- No schema changes are required in `AgentConfig` or `AntigravityProviderConfig`.
