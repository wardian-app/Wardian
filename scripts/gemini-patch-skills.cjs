const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');

function commandNames(command, platform = process.platform) {
    if (platform === 'win32' && !/\.(exe|cmd|bat|ps1)$/i.test(command)) {
        return [`${command}.exe`, `${command}.cmd`, `${command}.bat`, command];
    }
    return [command];
}

function resolveCommand(command, env = process.env, platform = process.platform) {
    const pathValue = env.PATH || '';
    const entries = pathValue.split(path.delimiter).filter(Boolean);
    for (const dir of entries) {
        for (const name of commandNames(command, platform)) {
            const candidate = path.join(dir, name);
            if (fs.existsSync(candidate)) {
                return candidate;
            }
        }
    }
    return null;
}

function packageRootFrom(startPath) {
    let current = fs.existsSync(startPath) ? fs.realpathSync(startPath) : startPath;
    if (!fs.existsSync(current)) {
        return null;
    }
    if (!fs.statSync(current).isDirectory()) {
        current = path.dirname(current);
    }

    while (true) {
        const packageJson = path.join(current, 'package.json');
        if (fs.existsSync(packageJson)) {
            try {
                const pkg = JSON.parse(fs.readFileSync(packageJson, 'utf8'));
                if (pkg.name === '@google/gemini-cli' || pkg.name === 'gemini-cli') {
                    return current;
                }
            } catch {
                // Keep walking; malformed package metadata should not stop discovery.
            }
        }

        const parent = path.dirname(current);
        if (parent === current) {
            return null;
        }
        current = parent;
    }
}

function packageRootsFromManagers() {
    const managers = [
        ['npm', ['root', '-g']],
        ['pnpm', ['root', '-g']],
    ];
    const roots = [];
    for (const [manager, args] of managers) {
        const result = spawnSync(manager, args, {
            encoding: 'utf8',
            shell: process.platform === 'win32',
            stdio: ['ignore', 'pipe', 'ignore'],
            windowsHide: process.platform === 'win32',
        });
        if (result.status === 0 && result.stdout.trim()) {
            roots.push(result.stdout.trim());
        }
    }
    return roots;
}

function getCliDir() {
    const candidates = [];

    if (process.env.GEMINI_CLI_DIR) {
        candidates.push(process.env.GEMINI_CLI_DIR);
    }

    for (const root of packageRootsFromManagers()) {
        candidates.push(path.join(root, '@google', 'gemini-cli'));
        candidates.push(path.join(root, 'gemini-cli'));
    }

    const geminiCommand = resolveCommand('gemini');
    if (geminiCommand) {
        const packageRoot = packageRootFrom(geminiCommand);
        if (packageRoot) {
            candidates.push(packageRoot);
        }
    }

    // Backward-compatible fallback for the original Windows nvm layout.
    candidates.push('C:\\nvm4w\\nodejs\\node_modules\\@google\\gemini-cli');

    for (const candidate of candidates) {
        if (candidate && fs.existsSync(path.join(candidate, 'package.json'))) {
            return candidate;
        }
    }

    return null;
}

function findFiles(dir, filter) {
    let results = [];
    if (!fs.existsSync(dir)) return results;
    const list = fs.readdirSync(dir);
    for (const file of list) {
        const filePath = path.join(dir, file);
        const stat = fs.statSync(filePath);
        // Avoid deep traversal of node_modules unless it's our core package
        if (stat.isDirectory() && !filePath.endsWith('node_modules')) {
            results = results.concat(findFiles(filePath, filter));
        } else if (!stat.isDirectory() && filter(filePath)) {
            results.push(filePath);
        }
    }
    return results;
}

function main() {
    const cliDir = getCliDir();
    if (!cliDir) {
        console.error('Could not find gemini-cli installation.');
        process.exit(1);
    }

    console.log(`Found CLI dir at ${cliDir}`);

    // Support bundled (<=0.34.0-preview, >=0.35.0) and unbundled (0.34.0 stable) structures
    let searchDirs = [
        path.join(cliDir, 'bundle'),
        path.join(cliDir, 'dist'),
        path.join(cliDir, 'node_modules', '@google', 'gemini-cli-core', 'dist')
    ];

    let allJsFiles = [];
    for (const dir of searchDirs) {
        if (fs.existsSync(dir)) {
            allJsFiles = allJsFiles.concat(findFiles(dir, f => f.endsWith('.js')));
        }
    }

    let patchedFiles = 0;

    for (const file of allJsFiles) {
        let content = fs.readFileSync(file, 'utf8');
        let changed = false;

        if (content.includes('// Treat included directories as secondary workspaces')) {
            console.log(`Already patched: ${file}`);
            continue;
        }

        // 1. Patch discoverSkills method definition dynamically
        const sigRegex = /(async\s+discoverSkills\s*\([^)]*)(\)\s*\{)/;
        const anchorRegex = /(const\s+(\w+)\s*=\s*await\s+loadSkillsFromDir\(.*?getUserSkillsDir\(\)\);)/;

        if (sigRegex.test(content) && anchorRegex.test(content)) {
            console.log(`Patching discoverSkills definition in ${file}`);

            const pathAliasMatch = content.match(/const\s+builtinDir\s*=\s*(path\w*)\.join\(/);
            const pathAlias = pathAliasMatch ? pathAliasMatch[1] : 'path';

            // Update signature to accept new parameters
            content = content.replace(sigRegex, '$1, includeDirectories = [], loadFromIncludeDirectories = false$2');

            // Inject logic right before User Skills are loaded
            const injection = `
        // Treat included directories as secondary workspaces
        if (loadFromIncludeDirectories && includeDirectories.length > 0) {
          for (const dir of includeDirectories) {
            const geminiSkills = await loadSkillsFromDir(${pathAlias}.join(dir, ".gemini", "skills"));
            this.addSkillsWithPrecedence(geminiSkills);
            const agentSkills = await loadSkillsFromDir(${pathAlias}.join(dir, ".agents", "skills"));
            this.addSkillsWithPrecedence(agentSkills);
          }
        }
        $1`;
            content = content.replace(anchorRegex, injection);
            changed = true;
        }

        // 2. Patch call sites robustly
        // Matches up to the closing parenthesis before a semicolon or bracket
        const callSiteRegex = /await\s+this\.getSkillManager\(\)\.discoverSkills\([\s\S]*?\)(?=;|\s+\{)/g;
        if (callSiteRegex.test(content)) {
            console.log(`Patching discoverSkills call sites in ${file}`);
            content = content.replace(callSiteRegex, `await this.getSkillManager().discoverSkills(this.storage, this.getExtensions(), this.isTrustedFolder(), this.workspaceContext.getDirectories(), this.loadMemoryFromIncludeDirectories)`);
            changed = true;
        }

        if (changed) {
            fs.writeFileSync(file, content);
            patchedFiles++;
        }
    }

    if (patchedFiles > 0) {
        console.log(`Successfully patched ${patchedFiles} file(s).`);
    } else {
        console.log('No files needed patching. The environment might already be patched or the structure is unrecognized.');
    }
}

module.exports = {
    commandNames,
    findFiles,
    getCliDir,
    main,
    packageRootFrom,
    resolveCommand,
};

if (require.main === module) {
    main();
}
