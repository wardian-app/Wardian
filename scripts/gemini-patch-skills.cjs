const fs = require('fs');
const path = require('path');

function locateGeminiJs() {
    const candidates = [
        path.join(__dirname, 'bundle', 'gemini.js'),
        path.join(process.cwd(), 'bundle', 'gemini.js'),
    ];
    if (process.platform === 'win32' && process.env.APPDATA) {
        candidates.push(path.join(process.env.APPDATA, 'npm', 'node_modules', '@google', 'gemini-cli', 'bundle', 'gemini.js'));
    }
    for (const p of candidates) {
        if (fs.existsSync(p)) return p;
    }
    return null;
}

const geminiJsPath = locateGeminiJs();
if (!geminiJsPath) {
    process.exit(0);
}

let content = fs.readFileSync(geminiJsPath, 'utf8');

const pathAliasMatch = content.match(/const\s+builtinDir\s*=\s*(path\d+)\.join\(__dirname\d+,\s*"builtin"\)/);
const pathAlias = pathAliasMatch ? pathAliasMatch[1] : 'path70';

console.log('Applying skill discovery patch...');

// REPLACEMENT: Use an extremely broad regex to find and replace the ENTIRE discoverSkills method body.
// This ensures that any previous versions (including the "rootSkills" version) are completely wiped.
const discoverSkillsFullRegex = /async\s+discoverSkills\s*\([^)]*\)\s*\{[\s\S]*?this\.addSkillsWithPrecedence\(projectAgentSkills\);\s*\}/i;

const discoverSkillsFullReplacement = `async discoverSkills(storage2, extensions = [], isTrusted = false, includeDirectories = [], loadFromIncludeDirectories = false) {
        this.clearSkills();
        await this.discoverBuiltinSkills();
        for (const extension of extensions) {
          if (extension.isActive && extension.skills) {
            this.addSkillsWithPrecedence(extension.skills);
          }
        }
        if (loadFromIncludeDirectories && includeDirectories.length > 0) {
          for (const dir of includeDirectories) {
            // Included folders: .gemini/skills/ and .agents/skills/ only
            const geminiSkills = await loadSkillsFromDir(${pathAlias}.join(dir, ".gemini", "skills"));
            this.addSkillsWithPrecedence(geminiSkills);
            const agentSkills = await loadSkillsFromDir(${pathAlias}.join(dir, ".agents", "skills"));
            this.addSkillsWithPrecedence(agentSkills);
          }
        }
        const userSkills = await loadSkillsFromDir(Storage.getUserSkillsDir());
        this.addSkillsWithPrecedence(userSkills);
        const userAgentSkills = await loadSkillsFromDir(Storage.getUserAgentSkillsDir());
        this.addSkillsWithPrecedence(userAgentSkills);
        if (!isTrusted) {
          debugLogger.debug("Workspace skills disabled because folder is not trusted.");
          return;
        }
        const projectSkills = await loadSkillsFromDir(storage2.getProjectSkillsDir());
        this.addSkillsWithPrecedence(projectSkills);
        const projectAgentSkills = await loadSkillsFromDir(storage2.getProjectAgentSkillsDir());
        this.addSkillsWithPrecedence(projectAgentSkills);
      }`;

if (!discoverSkillsFullRegex.test(content)) {
    console.error('Error: Could not locate the discoverSkills method in bundle/gemini.js');
    process.exit(1);
}

content = content.replace(discoverSkillsFullRegex, discoverSkillsFullReplacement);

// Re-verify call sites (idempotent)
const callSiteRegex = /await\s+this\.getSkillManager\(\)\.discoverSkills\([\s\S]*?\)(?=;|\s+\{)/g;
content = content.replace(callSiteRegex, `await this.getSkillManager().discoverSkills(this.storage, this.getExtensions(), this.isTrustedFolder(), this.workspaceContext.getDirectories(), this.loadMemoryFromIncludeDirectories)`);

fs.writeFileSync(geminiJsPath, content);
console.log('Successfully applied skill discovery patch.');
