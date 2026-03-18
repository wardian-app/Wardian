# Spec 004: Wardian Library System (Prompts & Skills)

**Status**: Proposed  
**Date**: 2026-03-15  
**Context**:  
Wardian requires a centralized, high-efficiency repository for managing reusable **Prompts** and **Skills**. Users need to organize these assets logically, discover favorites rapidly, and inject them into active agent sessions without friction.

## Decision

1. **Unified Class Registry (`classes.json`)**:
   - Consolidate `default_classes.json` and `custom_classes.json` into a single source of truth: `C:\Users\tgemi\.wardian\classes.json`.
   - The backend will perform a one-time merge of defaults into this file if it doesn't exist.
   - Every class will support an `assigned_skills: string[]` field.

2. **Library Storage Model (Physical-First)**:
   - **Prompts**: Stored as `.md` files in `C:\Users\tgemi\.wardian\library\prompts\`.
   - **Folders**: Physical directories on disk define the organizational hierarchy.
   - **Metadata**: A `library.json` sidecar file in the library root will store "Virtual" metadata (Tags, Stars/Favorites) keyed by the relative file path.

3. **Workspace Integration**:
   - **Main View**: `LibraryView.tsx` will be a primary workspace view for management, folder navigation, and tagging.
   - **Injection Hub**: The **Command Sidebar Tab** (`CommandPanel.tsx`) will be refactored to display Starred Prompts for immediate injection into the focused terminal.

5. **Skill Deployment Model (Direct Copying)**:
   - To ensure strict agent isolation and avoid "bleedover" between sessions, skills will be **physically copied** (recursively) from the Library to the target entity's `.agents/skills/` directory upon assignment.
   - **Precedence**: This ensures that an agent spawned from a class "inherits" its own local, immutable snapshot of that skill.
   - **Updates**: Updating a skill in the Library will *not* automatically update running agents; a deliberate "Re-deploy" action will be required to refresh an agent's or class's skill set.

6. **Technical Implementation**:
   - New Tauri command `inject_session_input(session_id: String, text: String)` will write bytes directly to the process stdin followed by `\n`.

## Consequences

- **Pros**: 
  - Strict data isolation via the physical folder structure.
  - High visibility for favorites across all views (Grid/Dashboard).
  - Simplified class management via a single JSON registry.
- **Cons**: 
  - Requires a migration path for existing `custom_classes.json`.
  - Metadata in `library.json` must be kept in sync if files are moved externally (mitigated by recommending in-app organization).

## Schema Definitions

### Library Item Metadata
```typescript
interface LibraryItemMetadata {
    id: string; // UUID
    tags: string[];
    is_starred: boolean;
    last_used?: string;
}
```

### Unified Class Definition
```json
{
  "name": "Architect",
  "description": "...",
  "assigned_skills": ["playwright-cli"]
}
```
