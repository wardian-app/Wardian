# Fresh Astra messaging experiment

Date: 2026-09-07. Tracking issue: [#1218](https://github.com/wardian-app/Wardian/issues/1218).

This report preserves the initial v1-shaped `send_input` experiment. The
subsequent implementation direction targets Codex's v2 collaboration tools;
the results below do not establish v2 behavioral compatibility.

## Result

A fresh `gpt-6-astra` session discovered and called Wardian's `send_input` MCP
tool from a plain-English coordination task. The successful trial delivered
the exact message through Wardian to a real OpenCode agent, which completed
its response. Astra used ordinary, complete prose: 77 whitespace-separated
words and 521 characters. This experiment did not elicit the compressed
coordination style observed in an existing long-running agent session.

This is evidence that the small tool surface is discoverable and functional.
It does not show that familiar arguments alone reproduce built-in subagent
language, and does not establish a training-related cause. No handler
replacement has been implemented or selected by this result.

## Setup

- Sender: Codex CLI 0.153.4, `gpt-6-astra`, low reasoning effort, fresh
  app-server thread. The live model catalog confirmed that exact model and
  effort before any model turn.
- Recipient: a disposable Wardian agent named `Release-Check`, OpenCode
  `opencode/mimo-v2.5-free`, using the native ACP transport.
- Sender isolation: empty workspace outside repository ancestry; new provider
  home; no parent thread fork, prior sessions, project AGENTS.md, or Wardian
  memories. Only authentication was copied, then removed from the test home.
  Normal provider instructions and six bundled system skills remained. The
  skill inventory contained no Wardian or Engage skills.
- Tool exposure: one Wardian MCP tool, `send_input`, with `target`, `message`,
  and optional `interrupt`. The schema said interruption was unsupported.
  Built-in subagents were disabled, so this is not a comparison of preference
  between competing delegation systems.
- Validation: a separate literal-text MCP call first proved Unicode,
  punctuation, and final-newline preservation in the native broker envelope.
  The Astra call then required a real submission ID, exact recipient and body,
  and completed native delivery. The recipient's response was inspected
  separately; the first tool does not return that response to Astra.

## Exact task

> Please contact Release-Check and ask them to assess this change: payment requests would retry five times instead of three after a timeout, but the requests do not include an idempotency key. We need their recommendation before Friday's release.

The task contained no tool name, example message, compression request, or
reference to token efficiency. The subject was a hypothetical coordination
exercise; no payment system was accessed.

## Observations

| Trial | Tool discovery | Delivery | Message form |
| --- | --- | --- | --- |
| Initial provider trial | Correct tool and recipient | Codex rejected the call before submission because its approval policy was `never` | Ordinary prose, 75 words, 511 characters |
| Trial with explicit test-scoped tool permission | Correct tool and recipient | MCP call completed; real Wardian interaction reached native completion | Ordinary prose, 77 words, 521 characters |

The second trial used Codex's supported
`mcp_servers.wardian.tools.send_input.approval_mode = "approve"` setting only
inside the isolated test host, consistent with the user's authorization to
run the messaging experiment. The plain-English task and tool schema were
unchanged. It used a new thread; the failed message was not replayed into the
first sender's conversation. The initial failure and its language result were
retained. An earlier WebDriver configuration failure launched no provider.

The successful trial's exact tool message was:

> Please assess this proposed change before Friday's release (September 11): payment requests would retry five times instead of three after a timeout, and the requests do not include an idempotency key. We need your recommendation before the release. Please give a go/no-go recommendation, assess the risk of duplicate payments when a timed-out request may already have succeeded, and identify any required safeguards or validation before shipping. Please reply with your assessment and flag any missing information needed.

The message retained the retry counts, timeout trigger, absent idempotency
key, and deadline. It added useful questions about duplicate payments and
safeguards, but also expanded the wording. Astra correctly reported that the
request was sent and the recommendation remained pending; it did not invent
receipt of the recipient's answer. The recipient subsequently produced an
assessment through Wardian's native delivery record.

## Evidence and limits

The second invocation passed both its approval-result regression and real
native experiment. Private reports retain exact visible events and completed
delivery records. Published evidence excludes authentication, personal paths,
and private provider reasoning. Counts above are word and character counts,
not tokenizer measurements.

Artifact fingerprints for the successful run:

| Artifact | SHA-256 |
| --- | --- |
| Messaging CLI | `de2a4a6715f52e6d99bbddcab2e89d253bd9ee555fd329f5ac86183fdeedbcfb` |
| Packaged Wardian integration-v8 app | `fb4d590c87e0a08ecf6af077144cd3e2cbcca1fcd7e7e05eaa6a0e38cbe9c8f2` |
| Codex executable | `444a3f0008050605cae73cd9b7a2dcac61294062dfaab56dd20430fd6498518b` |
| Native experiment source | `6a97f347426ef4c0d0c4e1c67c4eed4ed750ab07b6569534f4111a85551639d1` |

The app artifact includes separately tracked provider-conformance fixes; this
run is not evidence that those fixes are merged or deployed. The tool adapter
also has real CLI-child/control-endpoint tests independent of that integration
app. This run exercised a background sender and native recipient, not a live
terminal sender. No automatic notification, reply, or interruption parity was
claimed.

Two low-effort trials on one task cannot establish how all Astra sessions
will communicate. A later controlled comparison could vary the tool
description, expose additional delegation context/capabilities, or compare an
actual built-in send handler while holding task and initial context constant.
Changing several of those variables together would obscure which change
affected behavior.

## Subsequent interpretation

Inspection of the original long-running session found ordinary English and,
about nine hours later, concatenated shorthand in outgoing calls to the same
built-in `send_input` tool. The shorthand was already present in the generated
arguments; the transport did not introduce it. This supports investigating
style drift during a long task, but does not isolate context accumulation,
compaction, repetition, or a training-related cause. Token efficiency was not
measured. Reproducing compressed spelling is not an acceptance criterion for
the v2 integration; tool selection, delivery semantics, and receiving behavior
must be tested directly.
