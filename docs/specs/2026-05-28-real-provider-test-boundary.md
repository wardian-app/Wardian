# Real Provider Test Boundary

## Context

Wardian supports deterministic mock-provider tests for app-owned behavior and
opt-in real-provider tests for provider-owned runtime behavior. These layers
must stay separate. A mock provider can prove that Wardian routes a message,
records a delivery event, renders terminal output, or updates state, but it
cannot prove that a real provider CLI accepts PTY input, exposes a ready prompt,
clears its compose field, resumes a session, or responds through its real
transcript/runtime path.

## Rule

Provider-related claims require actual providers. If a test says Codex, Claude,
Gemini, OpenCode, or Antigravity behavior works, it must launch that provider's
real CLI in native E2E or be marked as skipped with `// @real-provider-only`.

Mock-backed tests may cover only Wardian-owned contracts:

- control endpoint routing
- mailbox persistence and delivery event recording
- queue-policy decisions
- frontend rendering and navigation
- deterministic PTY plumbing
- workflow execution against a simulated provider

Mock-backed tests must not spoof a real provider identity to validate
provider-specific readiness, terminal input, output parsing, prompt clearing,
or live communication.

## Real Provider Delivery

Use `e2e-native/tests/provider-delivery-real-native.test.mjs` for real delivery
validation. It is opt-in because it sends prompts to live provider accounts:

```bash
WARDIAN_E2E_REAL_DELIVERY=1 WARDIAN_E2E_DELIVERY_PROVIDERS=codex,claude,gemini,opencode,antigravity npm run test:e2e:native:fast -- e2e-native/tests/provider-delivery-real-native.test.mjs
```

PowerShell:

```powershell
$env:WARDIAN_E2E_REAL_DELIVERY = "1"
$env:WARDIAN_E2E_DELIVERY_PROVIDERS = "codex,claude,gemini,opencode,antigravity"
npm run test:e2e:native:fast -- e2e-native/tests/provider-delivery-real-native.test.mjs
Remove-Item Env:\WARDIAN_E2E_REAL_DELIVERY
Remove-Item Env:\WARDIAN_E2E_DELIVERY_PROVIDERS
```

The default case is one short mailbox-only prompt per provider. Use
`WARDIAN_E2E_DELIVERY_CASES=all` for the full input case set, or a comma list
such as `mailbox-short,mailbox-multiline`.

## Cheap Models

Use cheap or fast model settings where the provider exposes a stable model flag.
The real delivery test applies these defaults:

- Claude: `haiku`
- Gemini: `gemini-2.5-flash`
- OpenCode: `opencode/deepseek-v4-flash-free`

Override any provider with:

```bash
WARDIAN_E2E_DELIVERY_CLAUDE_MODEL=<model>
WARDIAN_E2E_DELIVERY_GEMINI_MODEL=<model>
WARDIAN_E2E_DELIVERY_OPENCODE_MODEL=<model>
```

PowerShell:

```powershell
$env:WARDIAN_E2E_DELIVERY_CLAUDE_MODEL = "<model>"
$env:WARDIAN_E2E_DELIVERY_GEMINI_MODEL = "<model>"
$env:WARDIAN_E2E_DELIVERY_OPENCODE_MODEL = "<model>"
```

Set a provider-specific model variable to an empty value to use that provider's
default model. For providers without a model flag, use provider-specific custom
args only when the provider supports them:

```bash
WARDIAN_E2E_DELIVERY_ANTIGRAVITY_ARGS="--some-provider-arg value"
```

## Review Checklist

When adding or reviewing a provider-related test, ask:

- Does this test launch the real provider binary?
- Does it use the provider's actual terminal/runtime path?
- Does it depend on provider auth, account state, or model behavior?
- If it uses the mock provider, is the assertion limited to Wardian-owned logic?
- Is any provider-specific gap marked `// @real-provider-only` instead of
  silently represented by a mock?

If the answer is unclear, the test name or comments must be tightened before
the test is accepted.
