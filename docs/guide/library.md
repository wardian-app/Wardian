# The Library System

The Library is the centralized repository for all reusable agent assets in Wardian. It allows you to manage "Blueprints" (what an agent can be) and "Actions" (what an agent can do).

![Wardian Library view showing folders, reusable prompts, starred prompts, and search controls](../assets/screenshots/library/library-view.png)

## 🗂️ Library Sections

### 1. Prompts

Prompts are reusable text injections that you can send directly to an agent's terminal.

- **Organization**: Prompts are stored as `.md` files in `<wardian-home>/library/prompts/`. You can organize them into physical folders on your disk.
- **Quick Injection**: "Star" your favorite prompts to make them appear in the **Command** sidebar tab for one-click execution.
- **Dynamic Context**: Use prompts to quickly set up environments, run test suites, or provide complex task instructions.

### 2. Skills

Skills are modular capabilities (extensions) that can be deployed to your agents or classes.

- **Physical Deployment**: Unlike a simple configuration toggle, skills in Wardian use **Windows Junction Points** or Unix symlinks. If link creation fails, Wardian falls back to a recursive copy.
- **Live Sync**: When you edit a skill's source code in the Library, every agent or class that has that skill linked will receive the update instantly unless that deployment used the fallback copy path.
- **Target Scopes**:
  - **Global**: Deploys the skill to all agents.
  - **Class**: Deploys the skill to a specific blueprint (e.g., all future `Coder` agents).
  - **Instance**: Deploys the skill only to one specific, active agent session.

### 3. Classes

Classes are the foundational blueprints for your agents.

- **Consolidated Registry**: All classes are stored in a single `classes.json` file in your `.wardian` folder.
- **Custom Instructions**: Edit the `AGENTS.md` content for any class to define its personality, constraints, and standard operating procedures.
- **Default Skills**: Pre-assign specific skills to a class so every instance spawned from it starts with the necessary tools.

## 🖱️ Key Interactions

### Managing Assets

- **Right-Click**: Use the context menu on any item to **Delete**, **Rename**, or **Reveal in System Explorer**.
- **Metadata Editor**: Click an item to open the editor. Here you can add **Tags** for easy searching and toggle the **Star** status for favorite items.

### Running Prompts

To run a prompt from the Library:

1. Select one or more agents in the **Roster** (Right Sidebar).
2. Find the prompt in the **Library**.
3. Click the **Run** icon (Play button). The prompt text will be flattened into a single line and sent to the selected terminals automatically.

## 🚀 Advanced: Provider Skill Discovery

Wardian adapts the same assigned skills to each provider's native discovery model:

- Gemini uses Wardian's Gemini patch so `--include-directories` can expose common, class, and agent skill roots.
- Claude uses additional instruction roots and `.claude/skills` links where provider-native discovery requires them.
- Codex receives scoped skills in the agent-specific `CODEX_HOME/skills` habitat.
- OpenCode receives scoped skills through Wardian's generated OpenCode config directory.

If Gemini skills are missing, ensure **Auto-patch Gemini CLI** is enabled in the **Settings** panel or run the patch manually. For other providers, start with the provider comparison in [Provider Runtimes](../providers.md).

## Related Research

- [Skill Manager References](../research/skill-manager-references.md)
