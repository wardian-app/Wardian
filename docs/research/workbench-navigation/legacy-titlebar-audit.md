# Legacy titlebar navigation audit

This audit freezes the legacy desktop surface-launch selectors that existed before
the workbench migration. It counts matching **paths**, not matching lines, so a
suite with several fixed-titlebar clicks remains one migration entry.

## Audited regex and baseline

The exact extended regular expression is:

```text
\.titlebar-(center|tab)|getByRole\("button", \{ name: "(Grid|Dashboard|Queue|Library|Workflows|Graph|Garden)"|normalize-space\(\.\)='(Grid|Dashboard|Queue|Library|Workflows|Graph|Garden)'
```

Run it from the repository root against the frozen baseline:

```bash
pattern='\.titlebar-(center|tab)|getByRole\("button", \{ name: "(Grid|Dashboard|Queue|Library|Workflows|Graph|Garden)"|normalize-space\(\.\)='"'"'(Grid|Dashboard|Queue|Library|Workflows|Graph|Garden)'"'"''
git grep -l -E "$pattern" d53842dc -- e2e e2e-native scripts src .github
```

PowerShell:

```powershell
$pattern = '\.titlebar-(center|tab)|getByRole\("button", \{ name: "(Grid|Dashboard|Queue|Library|Workflows|Graph|Garden)"|normalize-space\(\.\)=''(Grid|Dashboard|Queue|Library|Workflows|Graph|Garden)'''
$paths = git grep -l -E $pattern d53842dc -- e2e e2e-native scripts src .github
$paths
$paths.Count
```

The command resolves commit `d53842dc` and returns exactly **25 paths**. The
baseline is intentionally immutable; run the same regex without the revision to
audit the current tree.

The migrated desktop suites use the deterministic flagged configuration
`e2e/playwright.workbench.config.ts`. That configuration sets
`VITE_WARDIAN_WORKBENCH=1` on its Vite server and disables server reuse so a
flag-off development server cannot satisfy the run accidentally.

## Baseline path disposition

| Baseline path | Disposition |
| --- | --- |
| `e2e/tests/agent-lifecycle.spec.ts` | Migrated in Task 15 to the semantic workbench helper. |
| `e2e/tests/critical-flows.spec.ts` | Migrated in Task 15 to the semantic workbench helper. |
| `e2e/tests/features.spec.ts` | Migrated in Task 15 to the semantic workbench helper. |
| `e2e/tests/garden.spec.ts` | Migrated in Task 15 to the semantic workbench helper. |
| `e2e/tests/graph-topology.spec.ts` | Migrated in Task 15 to the semantic workbench helper. |
| `e2e/tests/library-redesign.spec.ts` | Migrated in Task 15 to the semantic workbench helper. |
| `e2e/tests/queue-v2.spec.ts` | Migrated in Task 15 to the semantic workbench helper. |
| `e2e/tests/run-params.spec.ts` | Migrated in Task 15 to the semantic workbench helper. |
| `e2e/tests/run-view.spec.ts` | Migrated in Task 15 to the semantic workbench helper. |
| `e2e/tests/schedule-monitor.spec.ts` | Migrated in Task 15 to the semantic workbench helper. |
| `e2e/tests/workflow-builder.spec.ts` | Migrated in Task 15 to the semantic workbench helper. |
| `e2e/tests/workflow.spec.ts` | Migrated in Task 15 to the semantic workbench helper. |
| `e2e/tests/workflows.spec.ts` | Migrated in Task 15 to the semantic workbench helper. |
| `e2e-native/tests/real-provider-rendering-native.test.mjs` | Scheduled for the Task 17 native semantic helper migration. |
| `e2e-native/tests/terminal-geometry-sweep-native.test.mjs` | Scheduled for the Task 17 native semantic helper migration. |
| `e2e-native/tests/terminal-rendering-native.test.mjs` | Scheduled for the Task 17 native semantic helper migration. |
| `e2e-native/tests/terminal-visibility-snapshot-native.test.mjs` | Scheduled for the Task 17 native semantic helper migration. |
| `e2e-native/tests/terminal-wheel-scroll-native.test.mjs` | Scheduled for the Task 17 native semantic helper migration. |
| `scripts/measure-view-performance.mjs` | Removed in Task 18 after replacement by the fail-closed production workbench performance harness. |
| `scripts/capture-doc-screenshots.mjs` | Scheduled for the Task 20 semantic capture migration. |
| `scripts/capture-readme-demo-real.mjs` | Scheduled for the Task 20 semantic capture migration. |
| `src/styles/App.css` | Retained only for the flag-off comparison; Task 19 removes the legacy titlebar navigation styles after the cutover gates pass. |
| `e2e/tests/remote-pwa.spec.ts` | Intentionally unrelated: this is the remote PWA's own mobile Queue navigation. |
| `src/features/remote/RemoteMobileApp.test.tsx` | Intentionally unrelated: these buttons belong to the remote mobile surface. |
| `src/features/settings/SettingsModal.test.tsx` | Intentionally unrelated: Queue and Grid are Settings navigation/density controls, not desktop surface launchers. |

No baseline hit may disappear from the current-tree audit without moving to its
recorded disposition. New matches introduced after `d53842dc` must be classified
separately by Task 19's cutover verifier; legitimate examples such as an Agents
Overview `Grid` mode control are not legacy desktop surface launchers. The three
baseline exceptions above remain allowed only for their recorded reasons.
