# Class Management

In Wardian, a **Class** is more than just a label—it is a functional blueprint that defines the core identity, intelligence, and equipment of your agents.

Classes are Wardian's reusable agent blueprints. They turn repeated setup
choices into editable, inspectable context so you can evolve an agent role over
time instead of copying the same instructions into every new session.

> **Class management has moved.** Creating, editing, and deleting classes no
> longer happens in a left-rail **Classes** tab — that panel has been
> retired. Class management now lives in the **Classes** section of the
> [Library](./library.md#3-classes). See that guide for the full workflow
> (the class workbench, editing `AGENTS.md`, managing deployed skills, and
> resetting or deleting a class). This page keeps only the conceptual
> material — what a class is and how to spawn an agent from one.

## When to Use It

- Create a repeatable role such as `Coder`, `Reviewer`, or `Researcher`.
- Tune an existing role before spawning more agents from it.
- Assign skills at the class level so every new instance starts with the same capabilities.

## What You Can Do in the Library's Classes Section

The [Library](./library.md)'s Classes section is where all class editing
happens:

- Create, open, and browse classes (classes are flat — no folder nesting).
- Edit a class's `AGENTS.md` instructions in the same inline markdown editor
  every other Library section uses.
- View and manage the skills deployed to the class, including a per-skill
  remove control.
- See provider defaults (default vs. custom class, and its description).
- **Reset to default** for built-in classes, or **Delete class** for custom
  ones.

See [The Library System](./library.md#3-classes) for the complete, up-to-date
walkthrough of this workbench.

## Configuring a Blueprint

### 1. Instruction Set (AGENTS.md)
Each class is governed by a markdown file. This is where you define:
- **Role & Personality**: Who the agent is (e.g., "A skeptical security auditor").
- **Constraints**: What the agent cannot do (e.g., "Never overwrite existing .env files").
- **Standard Procedures**: How the agent should approach tasks (e.g., "Always draft a plan before executing").

### 2. Pre-Assigned Skills
You can pre-load a class with specific modular skills.
- When you assign a skill to a class, every agent spawned from that blueprint receives the skill through its provider's discovery path. Wardian may use directory links, provider home projection, generated config, or provider-native include roots depending on the selected CLI.
- This ensures your `Coder` class always starts with `github-cli` and `typescript-tools` ready to go.

### 3. Registry Persistence
All your custom classes are stored in `<wardian-home>/classes.json`. This single file is the source of truth for the Rust backend when spawning new sessions.

## Spawning from a Class
To use your blueprint:
1. Navigate to the **Agent Configuration** tab in the Left Sidebar.
2. Select your class from the dropdown.
3. Click **Initialize**.
The agent will inherit the selected class instructions and class-level skill assignments.

## Important Limits

- Editing a class changes future spawns; it does not rewrite instructions inside already-running provider sessions.
- Class skills are adapted to each provider's discovery model. Check [Provider Runtimes](../providers.md) when a provider does not expose a skill as expected.
- Keep class instructions durable and general. Put task-specific instructions in prompts, broadcasts, workflows, or direct terminal messages.
- Treat class edits as shared capability changes. When an instruction is only
  useful for one project, agent, or workflow run, keep it in that narrower scope
  until it proves reusable.

## Related Links

- [Getting Started](./getting-started.md)
- [Library](./library.md)
- [Grid](./grid.md)
- [Provider Runtimes](../providers.md)
