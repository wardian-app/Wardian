# Custom Agent Classes

Wardian allows you to define specialized **Agent Classes** to give your agent swarm tailored identities and system-wide mandates.

## 🛠️ Creating a Custom Class

1. Open the **Left Sidebar (Explorer)** and select the **Class Manager** icon.
2. Fill out the **Create Class** form:
    - **Name**: A clear identifier (e.g., "DevOps", "Tester").
    - **Description**: A short summary of the agent's role.
    - **GEMINI.md**: The foundational system prompt for the agent. This defines its identity, mission, and tool preferences.

### What goes in GEMINI.md?
The `GEMINI.md` is where you define the agent's personality and instructions. It should follow this standard structure:
```markdown
# Role: [Class Name]

Define the mission and expertise of the agent.

## Core Mandates
- Rule 1
- Rule 2

## Capabilities & Tool Usage
- Preferred tools and their specific application.
```

## 📂 Automatic Directory Setup
When a custom class is created, Wardian automatically:
1. Creates a specialized directory in `~/.wardian/classes/[Class_Name]/`.
2. Populates it with your provided `GEMINI.md`.
3. Makes the class available in the **Spawn Instance** form.

## 🗑️ Managing Classes

### Default Classes
Wardian comes with a set of built-in classes (Architect, Coder, Researcher, etc.) that are read-only and provide a solid foundation for any project.

### Custom Classes
- **Deleting**: You can delete any custom class by clicking the trash icon in the Class Manager list.
- **Editing**: To edit a class, modify the `GEMINI.md` file located in its `~/.wardian/classes/` directory. Agents spawned from this class will automatically pick up the new instructions on their next restart.

## 🤖 Precedence of Instructions
When an agent starts, it combines instructions from multiple sources in this priority order:
1. **User Request**: The immediate task at hand.
2. **Project `GEMINI.md`**: Global project-level rules (found in the project root).
3. **Class `GEMINI.md`**: The role-specific foundation (defined in the Class Manager).
