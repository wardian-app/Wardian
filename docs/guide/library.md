# The Library System

The Library is the centralized repository for all reusable agent assets in Wardian. It allows you to manage "Blueprints" (what an agent can be) and "Actions" (what an agent can do).

## 🗂️ Library Sections

### 1. Prompts

Prompts are reusable text injections that you can send directly to an agent's terminal.

- **Organization**: Prompts are stored as `.md` files in `~/.wardian/library/prompts/`. You can organize them into physical folders on your disk.
- **Quick Injection**: "Star" your favorite prompts to make them appear in the **Command** sidebar tab for one-click execution.
- **Dynamic Context**: Use prompts to quickly set up environments, run test suites, or provide complex task instructions.

### 2. Skills

Skills are modular capabilities (extensions) that can be physically deployed to your agents or classes.

- **Physical Deployment**: Unlike a simple configuration toggle, skills in Wardian use **Windows Junction Points** (or Symlinks on Unix).
- **Live Sync**: When you edit a skill's source code in the Library, every agent or class that has that skill "Linked" will receive the update instantly.
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

## 🚀 Advanced: Skill Auto-Patching

If you use custom modular skills, ensure the **"Auto-patch Gemini CLI"** setting is enabled in the **Settings** panel. This ensures the underlying CLI is patched at launch to recognize the physical skill folders in your agent directories.
