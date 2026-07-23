# Key Concepts

Wardian has a few connected parts. This page names them in the same way you
see them in the app, so you can tell what you are opening, changing, or
sharing.

## The App Layout

```text
Wardian window
├── Left sidebar
│   └── Tools such as Explorer, Source Control, Command, and Settings
├── Workbench
│   └── Pane (a split region)
│       └── Tab (an open view)
│           └── Content such as an agent, Dashboard, Inbox, or Library
└── Right roster
    └── Watchlists, teams, and the agents you can monitor or target
```

```text
┌──────────────┬───────────────────────────────┬────────────────┐
│ Left sidebar │ Workbench                     │ Right roster   │
│              │ ┌───────────┬───────────────┐ │ Watchlist: All │
│ Explorer     │ │ Pane 1    │ Pane 2        │ │ ├─ Build team  │
│ Command      │ │ [Agent A] │ [Dashboard]   │ │ │  ├─ Agent A  │
│ Settings     │ │           │               │ │ │  └─ Agent B  │
│              │ └───────────┴───────────────┘ │ └─ Agent C     │
└──────────────┴───────────────────────────────┴────────────────┘
```

| Term | What it means |
| --- | --- |
| **Left sidebar** | Tools that act on the current selection without replacing what is open in the center. |
| **Workbench** | The central area where you arrange the views you are using. |
| **Pane** | One region of the Workbench. Split a pane to work side by side. |
| **Tab** | One open view inside a pane. Move, close, or reopen tabs without stopping an agent. |
| **Right roster** | The persistent list used to monitor agents and choose the agents that tools should target. |

## How an Agent Is Composed

An agent is a working instance of a coding assistant, not a terminal tab or a
row in the roster. It brings together the context for this task, a reusable
setup, and a live provider session.

```text
Agent
├── This instance
│   ├── Name, agent ID, provider, and model
│   └── Provider choices such as approvals and sandboxing
├── Task context
│   ├── Workspace: the main project folder
│   ├── Project instructions and files in that workspace
│   ├── Additional folders it may read (optional)
│   └── Worktree: an optional isolated Git checkout for branch work
├── Reusable practice
│   ├── Skills
│   │   ├── Shared with your user profile
│   │   ├── Given to the agent's class
│   │   └── Assigned directly to this agent
│   └── Class: a reusable role for new agents
│       ├── Instructions (AGENTS.md)
│       └── Skills for that role
├── Live work
│   ├── Provider session and conversation
│   ├── Terminal and Chat: two views of that work
│   └── Conversation archive, when enabled
└── Result and observation
    ├── Status, current activity, and telemetry
    ├── Inbox items for finished work, updates, approvals, or actions
    └── Roster, teams, watchlists, and Workbench tabs that refer to the agent
```

Some Wardian objects compose with an agent without becoming part of it:

```text
Prompt          → a saved message sent to one or more agents
Workflow        → a reusable process that can coordinate agent work
Team/watchlist  → a working set of agents for people to monitor or target
Inbox/evidence  → records the outcome of agent or workflow work
Workbench tab   → one presentation of a running agent
```

Those objects are deliberately separate. A class changes the reusable setup
for new agents; a prompt is an on-demand message; a workflow is a process that
may use agents. Closing a tab or removing an agent from a watchlist does not
stop its running session.

| Term | What it means |
| --- | --- |
| **Agent** | A named working instance with task context, selected reusable practice, and a provider session. |
| **Provider and model** | The coding tool the agent runs, plus the model and provider-specific choices it uses. |
| **Class** | A reusable role for new agents, with shared instructions and skills. It does not automatically change existing agents. |
| **Prompt** | A saved message you choose to send when the same request comes up again. It is not permanent agent setup. |
| **Skill** | Reusable guidance or a procedure. A skill can be scoped to your profile, a class, or one agent. |
| **Workspace** | The main project folder and its local context. It can have additional readable folders. |
| **Worktree** | An optional separate Git checkout, usually used when an agent needs its own branch. Moving an agent to one starts a fresh provider session. |
| **Session** | The agent's current provider process and conversation. Closing its tab does not stop the session. |
| **Terminal and Chat** | Two ways to view the same agent: the provider's live terminal or Wardian's structured messages and tool activity. |
| **Telemetry** | Live health and activity information such as status, query count, uptime, CPU, and memory. |

## Tailorability Slope

By *tailorability*, we mean how much you can adapt Wardian to the way you work.
A genuine slope trades a little more learning and effort for a little more
control at each step. You can stop anywhere, use something another person has
made, or move back down when a simpler option is enough.

```text
More tailoring power
      │                                      Build a new custom tool
      │                                      or integration (advanced)
      │                                Compose a workflow blueprint
      │                          Define a reusable role: Class
      │                    Write a reusable procedure: Skill
      │              Save a repeated request: Prompt
      │       Adjust an existing agent's workspace, model, or setup
      │ Use an existing agent, project, and built-in tools
      └──────────────────────────────────────────────────────────────
        Less learning and effort             More learning and effort
```

The point is not to turn every task into a workflow or every user into a
programmer. Use the smallest adaptation that solves the problem. Move upward
only when it cannot express the change you need.

| When you need | Smallest useful move | What becomes reusable |
| --- | --- | --- |
| A task that fits an existing setup | Start an agent with the right workspace, class, and provider | Nothing yet; use what exists. |
| A small task-specific adjustment | Change the agent's workspace, model, or direct setup | The agent's saved setup. |
| A request you keep rewriting | [Prompt](./library.md#2-prompts) | A message you can send again without making it permanent setup. |
| A procedure that should be followed consistently | [Skill](./library.md#1-skills) | Guidance that can be deployed to one agent, a class, or your user profile. |
| A role with a stable way of working | [Class](./library.md#3-classes) | Shared instructions and skills for new agents in that role. |
| A process with connected steps | [Workflow blueprint](./library.md#4-workflows) | A design that can produce separate workflow runs. |
| A need none of those pieces can express | A custom tool or integration | A new capability; this is an advanced, higher-effort step. |

Project files, instructions, results, and evidence stay alongside this slope.
They give each step the local context it needs, rather than becoming another
kind of reusable setup.

## Reusable Work

The [Library](./library.md) keeps the items you may want to use again. They
serve different purposes:

```text
Library
├── Prompts       → saved messages you send to selected agents
├── Skills        → reusable guidance deployed to an agent or class
├── Classes       → reusable starting setups for new agents
└── Workflows     → saved automation designs that can start workflow runs
```

A prompt is for a request you want to send again. A skill is for instructions
or a procedure an agent should have available while it works. A class combines
general instructions and skills for a repeatable role. A workflow coordinates
work across steps or agents.

## Workflow Runs

```text
Workflow blueprint (saved design)
├── Trigger or manual launch
├── Run (one execution of that design)
│   └── Nodes (the individual steps)
└── Results and history in Workflows and Inbox
```

Use a workflow blueprint when a process has repeatable steps. A run is one
specific time you execute it; changing a blueprint does not rewrite a run that
has already started.

## Where to Go Next

- [UI Overview](./ui-overview.md) explains the Workbench and sidebars in detail.
- [Agents](./agents-overview.md) explains monitoring and arranging agents.
- [Library](./library.md) explains prompts, skills, classes, and workflows.
- [Workflows](./workflows.md) explains building, launching, and monitoring workflow runs.
