# Gemini CLI Patches

This document tracks custom patches applied to the **Gemini CLI** bundle to enable missing features or fix upstream bugs that haven't been resolved in the official distribution.

## ⚠️ Important Note
These patches directly modify the `gemini.js` bundle within your `node_modules` (typically in `%APPDATA%\npm\node_modules\@google\gemini-cli`). 
**Upgrading the Gemini CLI will overwrite these changes.** Wardian provides a setting to automatically re-apply these patches upon application launch.

---

## 🛠️ Skill Discovery Patch (`gemini-patch-skills.cjs`)

### The Bug
By default, the Gemini CLI only looks for skills in the global `~/.gemini/skills` folder or the project's root `.gemini/skills` folder. It **ignores** any additional directories provided via the `--include-directories` flag.

### The Solution
The `gemini-patch-skills.cjs` script performs a surgical regex-based replacement on the `discoverSkills` method in `bundle/gemini.js`. This allows it to:
1. Iterate through the `include_directories` provided in the agent configuration.
2. Scan each directory for `.gemini/skills/` and `.agents/skills/`.
3. Load and register those skills with appropriate precedence.

### File Location
The script is located at: `D:\Development\Wardian\scripts\gemini-patch-skills.cjs`

### Usage in Wardian
To enable this patch at launch, toggle the **"Auto-patch Gemini CLI"** setting in the Wardian **Advanced Settings** panel.
