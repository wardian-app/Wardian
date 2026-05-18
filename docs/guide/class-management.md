# Class Management

In Wardian, a **Class** is more than just a label—it is a functional blueprint that defines the core identity, intelligence, and equipment of your agents.

## 🏛️ The Class Library
Class management has been relocated from the sidebar to the **Library** view to provide a spacious, professional editor for your blueprints.

1. Click **LIBRARY** in the top bar.
2. Select the **Classes** tab.
3. Browse the list of available classes or click **Create New Class**.

## 🔧 Configuring a Blueprint

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
All your custom classes are stored in `~/.wardian/classes.json`. This single file is the source of truth for the Rust backend when spawning new sessions.

## 🚀 Spawning from a Class
To use your blueprint:
1. Navigate to the **Agent Configuration** tab in the Left Sidebar.
2. Select your class from the dropdown.
3. Click **Spawn Instance**.
The agent will inherit all instructions and skills defined in the Library for that class.
