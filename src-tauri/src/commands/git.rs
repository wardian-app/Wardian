use crate::state::AppState;
use crate::utils::fs::create_directory_link;
use notify::Watcher;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tauri::{AppHandle, Emitter};
use wardian_core::models::git::{GitFileEntry, GitLogEntry, GitStatusResult};

/// Run a git command in the given directory and return stdout as a String.
///
/// Uses a direct command with `CREATE_NO_WINDOW` on Windows so stdout can be
/// captured without flashing a console window.
/// Sets `GIT_TERMINAL_PROMPT=0` and `GIT_ASKPASS=echo` so git never blocks
/// waiting for credential input (mirrors VS Code's git extension behaviour).
pub(crate) fn run_git(cwd: &str, args: &[&str]) -> Result<String, String> {
    run_git_with_candidates(cwd, args, &git_command_candidates())
}

fn run_git_allowing_status(
    cwd: &str,
    args: &[&str],
    allowed_statuses: &[i32],
) -> Result<String, String> {
    run_git_allowing_status_with_candidates(cwd, args, allowed_statuses, &git_command_candidates())
}

fn git_command_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![PathBuf::from("git")];

    #[cfg(unix)]
    {
        for path in ["/usr/bin/git", "/bin/git", "/usr/local/bin/git"] {
            let path = PathBuf::from(path);
            if path.is_file() {
                candidates.push(path);
            }
        }
    }

    #[cfg(windows)]
    {
        for path in [
            r"C:\Program Files\Git\cmd\git.exe",
            r"C:\Program Files\Git\bin\git.exe",
            r"C:\Program Files (x86)\Git\cmd\git.exe",
        ] {
            let path = PathBuf::from(path);
            if path.is_file() {
                candidates.push(path);
            }
        }
    }

    candidates
}

fn run_git_with_candidates(
    cwd: &str,
    args: &[&str],
    candidates: &[PathBuf],
) -> Result<String, String> {
    run_git_allowing_status_with_candidates(cwd, args, &[0], candidates)
}

fn run_git_allowing_status_with_candidates(
    cwd: &str,
    args: &[&str],
    allowed_statuses: &[i32],
    candidates: &[PathBuf],
) -> Result<String, String> {
    let mut last_not_found = None;

    for candidate in candidates {
        let output = match build_git_command(candidate, cwd, args).output() {
            Ok(output) => output,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                last_not_found = Some(error);
                continue;
            }
            Err(error) => return Err(format!("Failed to execute git: {}", error)),
        };

        let status_code = output.status.code();
        let status_allowed = status_code
            .map(|code| allowed_statuses.contains(&code))
            .unwrap_or(false);
        if !status_allowed {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(git_failure_message(
                status_code,
                stdout.as_ref(),
                stderr.as_ref(),
            ));
        }

        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }

    let message = last_not_found.map_or_else(
        || "no git command candidates configured".to_string(),
        |error| error.to_string(),
    );
    Err(format!("Failed to execute git: {}", message))
}

fn build_git_command(program: &Path, cwd: &str, args: &[&str]) -> Command {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "echo");

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }

    command
}

fn git_failure_message(status_code: Option<i32>, stdout: &str, stderr: &str) -> String {
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        return stderr.to_string();
    }

    let stdout = stdout.trim();
    if !stdout.is_empty() {
        return stdout.to_string();
    }

    match status_code {
        Some(code) => format!("git exited with status {}", code),
        None => "git exited unsuccessfully".to_string(),
    }
}

/// Parse `git status --porcelain=v1 -b` output into a GitStatusResult.
fn parse_porcelain_path(raw_path: &str) -> String {
    if let Some((_, new_path)) = raw_path.split_once(" -> ") {
        return new_path.to_string();
    }
    raw_path.to_string()
}

fn parse_tracking_name(tracking: &str) -> Option<String> {
    let name = tracking
        .split_once(" [")
        .map(|(value, _)| value)
        .unwrap_or(tracking)
        .trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Parse `git status --porcelain=v1 -b` output into a GitStatusResult.
fn parse_porcelain_status(raw: &str) -> GitStatusResult {
    let mut branch = String::new();
    let mut upstream = None;
    let mut ahead: u32 = 0;
    let mut behind: u32 = 0;
    let mut files = Vec::new();

    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            // Branch line: "## main...origin/main [ahead 1, behind 2]"
            let (branch_part, tracking) = match rest.find("...") {
                Some(idx) => (&rest[..idx], &rest[idx + 3..]),
                None => (rest, ""),
            };
            branch = branch_part.to_string();
            upstream = parse_tracking_name(tracking);

            if let Some(bracket_start) = tracking.find('[') {
                if let Some(bracket_end) = tracking.find(']') {
                    let info = &tracking[bracket_start + 1..bracket_end];
                    for part in info.split(',') {
                        let part = part.trim();
                        if let Some(n) = part.strip_prefix("ahead ") {
                            ahead = n.trim().parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix("behind ") {
                            behind = n.trim().parse().unwrap_or(0);
                        }
                    }
                }
            }
            continue;
        }

        if line.len() < 4 {
            continue;
        }

        let index_status = line.as_bytes()[0] as char;
        let worktree_status = line.as_bytes()[1] as char;
        let path = parse_porcelain_path(&line[3..]);

        // Staged entry (index has a real status, not ' ' or '?')
        if index_status != ' ' && index_status != '?' {
            files.push(GitFileEntry {
                path: path.clone(),
                status: index_status.to_string(),
                is_staged: true,
            });
        }

        // Unstaged / worktree entry
        if worktree_status != ' ' {
            let status = if index_status == '?' {
                "?".to_string()
            } else {
                worktree_status.to_string()
            };
            files.push(GitFileEntry {
                path,
                status,
                is_staged: false,
            });
        }
    }

    GitStatusResult {
        branch,
        has_upstream: upstream.is_some(),
        upstream,
        files,
        ahead,
        behind,
    }
}

#[tauri::command]
pub async fn git_status(cwd: String) -> Result<GitStatusResult, String> {
    let raw = run_git(
        &cwd,
        &["status", "--porcelain=v1", "-b", "--untracked-files=all"],
    )?;
    Ok(parse_porcelain_status(&raw))
}

#[tauri::command]
pub async fn git_current_branch(cwd: String) -> Result<String, String> {
    let branch = run_git(&cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    Ok(branch.trim().to_string())
}

#[tauri::command]
pub async fn git_log(cwd: String, count: u32) -> Result<Vec<GitLogEntry>, String> {
    let count_str = format!("-{}", count);
    let raw = run_git(
        &cwd,
        &[
            "log",
            &count_str,
            "--pretty=format:%H%n%s%n%an%n%ai",
            "--no-color",
        ],
    )?;

    let mut entries = Vec::new();
    let lines: Vec<&str> = raw.lines().collect();
    // Each entry is 4 lines: hash, message, author, date
    for chunk in lines.chunks(4) {
        if chunk.len() == 4 {
            entries.push(GitLogEntry {
                hash: chunk[0].to_string(),
                message: chunk[1].to_string(),
                author: chunk[2].to_string(),
                date: chunk[3].to_string(),
            });
        }
    }
    Ok(entries)
}

#[tauri::command]
pub async fn git_diff_file(cwd: String, path: String, staged: bool) -> Result<String, String> {
    if !staged && is_untracked_path(&cwd, &path)? {
        return run_git_allowing_status(
            &cwd,
            &["diff", "--no-color", "--no-index", "--", "/dev/null", &path],
            &[0, 1],
        );
    }

    let mut args = vec!["diff", "--no-color"];
    if staged {
        args.push("--cached");
    }
    args.push("--");
    args.push(&path);
    run_git(&cwd, &args)
}

#[tauri::command]
pub async fn git_stage(cwd: String, paths: Vec<String>) -> Result<(), String> {
    let mut args: Vec<&str> = vec!["add", "--"];
    for p in &paths {
        args.push(p);
    }
    run_git(&cwd, &args)?;
    Ok(())
}

#[tauri::command]
pub async fn git_unstage(cwd: String, paths: Vec<String>) -> Result<(), String> {
    let mut args: Vec<&str> = vec!["reset", "HEAD", "--"];
    for p in &paths {
        args.push(p);
    }
    run_git(&cwd, &args)?;
    Ok(())
}

#[tauri::command]
pub async fn git_discard_changes(cwd: String, paths: Vec<String>) -> Result<(), String> {
    // Separate tracked (checkout) from untracked (clean) files
    let status_raw = run_git(&cwd, &["status", "--porcelain=v1", "--untracked-files=all"])?;
    let mut tracked = Vec::new();
    let mut untracked = Vec::new();
    let path_set: std::collections::HashSet<&str> = paths.iter().map(|s| s.as_str()).collect();

    for line in status_raw.lines() {
        if line.len() < 4 {
            continue;
        }
        let file_path = parse_porcelain_path(&line[3..]);
        if !path_set.contains(file_path.as_str()) {
            continue;
        }
        if line.starts_with("??") {
            untracked.push(file_path);
        } else {
            tracked.push(file_path);
        }
    }

    if !tracked.is_empty() {
        let mut args: Vec<&str> = vec!["checkout", "--"];
        for p in &tracked {
            args.push(p);
        }
        run_git(&cwd, &args)?;
    }

    // Remove untracked files directly
    for p in &untracked {
        let full = std::path::Path::new(&cwd).join(p);
        if full.is_file() {
            let _ = std::fs::remove_file(&full);
        } else if full.is_dir() {
            let _ = std::fs::remove_dir_all(&full);
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn git_commit(cwd: String, message: String) -> Result<(), String> {
    run_git(&cwd, &["commit", "-m", &message])?;
    Ok(())
}

#[tauri::command]
pub async fn git_pull(cwd: String) -> Result<String, String> {
    run_git(&cwd, &["pull"])
}

#[tauri::command]
pub async fn git_push(cwd: String) -> Result<String, String> {
    if run_git(
        &cwd,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )
    .is_ok()
    {
        return run_git(&cwd, &["push"]);
    }

    let branch = run_git(&cwd, &["branch", "--show-current"])?;
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("Cannot publish detached HEAD.".to_string());
    }

    run_git(&cwd, &["push", "--set-upstream", "origin", branch])
}

fn is_untracked_path(cwd: &str, path: &str) -> Result<bool, String> {
    let raw = run_git(
        cwd,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--",
            path,
        ],
    )?;
    Ok(raw
        .lines()
        .any(|line| line.starts_with("?? ") && parse_porcelain_path(&line[3..]) == path))
}

#[tauri::command]
pub async fn git_create_worktree(cwd: String, path: String, branch: String) -> Result<(), String> {
    create_worktree_with_build_caches(Path::new(&cwd), Path::new(&path), &branch)
}

#[tauri::command]
pub async fn git_remove_worktree(cwd: String, path: String) -> Result<(), String> {
    remove_worktree(Path::new(&cwd), Path::new(&path))?;
    Ok(())
}

pub(crate) fn create_worktree_with_build_caches(
    workspace_path: &Path,
    worktree_path: &Path,
    branch: &str,
) -> Result<(), String> {
    let workspace_path = absolute_existing_path(workspace_path)?;
    let worktree_path = absolute_worktree_target_path(&workspace_path, worktree_path);
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let workspace = path_to_git_arg(&workspace_path)?;
    let worktree = path_to_git_arg(&worktree_path)?;
    run_git(&workspace, &["worktree", "add", &worktree, "-b", branch])?;

    let worktree_path = absolute_existing_path(&worktree_path)?;
    setup_worktree_build_caches(&worktree_path, &workspace_path)
}

pub(crate) fn remove_worktree(workspace_path: &Path, worktree_path: &Path) -> Result<(), String> {
    let workspace_path = absolute_existing_path(workspace_path)?;
    let worktree_path = absolute_worktree_target_path(&workspace_path, worktree_path);
    let workspace = path_to_git_arg(&workspace_path)?;
    let worktree = path_to_git_arg(&worktree_path)?;
    run_git(&workspace, &["worktree", "remove", &worktree, "--force"])?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitWorktreeInfo {
    pub path: PathBuf,
    pub branch: Option<String>,
}

pub(crate) fn parse_git_worktree_list_porcelain(raw: &str) -> Vec<GitWorktreeInfo> {
    let mut entries = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;

    for line in raw.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            if let Some(path) = current_path.take() {
                entries.push(GitWorktreeInfo {
                    path,
                    branch: current_branch.take(),
                });
            } else {
                current_branch = None;
            }
            continue;
        }

        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(previous_path) = current_path.replace(PathBuf::from(path)) {
                entries.push(GitWorktreeInfo {
                    path: previous_path,
                    branch: current_branch.take(),
                });
            }
            continue;
        }

        if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            current_branch = Some(branch.to_string());
        }
    }

    if let Some(path) = current_path {
        entries.push(GitWorktreeInfo {
            path,
            branch: current_branch,
        });
    }

    entries
}

pub(crate) fn list_git_worktrees(source_path: &Path) -> Result<Vec<GitWorktreeInfo>, String> {
    let source_path = absolute_existing_path(source_path)?;
    let workspace = path_to_git_arg(&source_path)?;
    let raw = run_git(&workspace, &["worktree", "list", "--porcelain"])?;
    Ok(parse_git_worktree_list_porcelain(&raw))
}

pub(crate) fn git_worktree_contains_path(
    source_path: &Path,
    worktree_path: &Path,
) -> Result<bool, String> {
    let source_path = absolute_existing_path(source_path)?;
    let expected = absolute_existing_path(worktree_path)
        .unwrap_or_else(|_| absolute_worktree_target_path(&source_path, worktree_path));
    let expected = normalize_path_for_compare(&expected);

    Ok(list_git_worktrees(&source_path)?.into_iter().any(|entry| {
        normalize_path_for_compare(&entry.path) == expected
    }))
}

fn absolute_worktree_target_path(workspace_path: &Path, worktree_path: &Path) -> PathBuf {
    if worktree_path.is_absolute() {
        worktree_path.to_path_buf()
    } else {
        workspace_path.join(worktree_path)
    }
}

fn path_to_git_arg(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(|value| value.to_string())
        .ok_or_else(|| format!("Path is not valid UTF-8: {}", path.display()))
}

pub(crate) fn setup_worktree_build_caches(
    worktree_path: &Path,
    workspace_path: &Path,
) -> Result<(), String> {
    let worktree_path = absolute_existing_path(worktree_path)?;
    let workspace_path = absolute_existing_path(workspace_path)?;

    if workspace_path.join("Cargo.toml").is_file() {
        write_cargo_worktree_config(&worktree_path, &workspace_path)?;
    }

    if workspace_path.join("package.json").is_file() {
        let target = ensure_shared_cache_dir(&workspace_path.join("node_modules"))?;
        ensure_worktree_cache_link(&target, &worktree_path.join("node_modules"))?;
    }

    if workspace_path.join("pyproject.toml").is_file()
        || workspace_path.join("requirements.txt").is_file()
    {
        let venv = workspace_path.join(".venv");
        if venv.is_dir() {
            let target = absolute_existing_path(&venv)?;
            ensure_worktree_cache_link(&target, &worktree_path.join(".venv"))?;
        }
    }

    Ok(())
}

fn write_cargo_worktree_config(worktree_path: &Path, workspace_path: &Path) -> Result<(), String> {
    let cargo_dir = worktree_path.join(".cargo");
    std::fs::create_dir_all(&cargo_dir).map_err(|e| e.to_string())?;

    let target_dir = workspace_path.join("target");
    let config = format!(
        "[build]\ntarget-dir = \"{}\"\n",
        escape_toml_basic_string(&target_dir.to_string_lossy())
    );
    std::fs::write(cargo_dir.join("config.toml"), config).map_err(|e| e.to_string())
}

fn ensure_shared_cache_dir(path: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
    absolute_existing_path(path)
}

fn ensure_worktree_cache_link(target: &Path, link: &Path) -> Result<(), String> {
    if link.exists() || link.symlink_metadata().is_ok() {
        if link_matches_target(link, target) {
            return Ok(());
        }
        return Err(format!(
            "Refusing to replace existing worktree cache path {}",
            link.to_string_lossy()
        ));
    }

    create_directory_link(target, link)
}

fn link_matches_target(link: &Path, target: &Path) -> bool {
    match (absolute_existing_path(link), absolute_existing_path(target)) {
        (Ok(link_path), Ok(target_path)) => link_path == target_path,
        _ => false,
    }
}

fn absolute_existing_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    normalize_canonical_path(&absolute)
}

fn normalize_canonical_path(path: &Path) -> Result<PathBuf, String> {
    let canonical = path.canonicalize().map_err(|e| e.to_string())?;

    #[cfg(windows)]
    {
        let text = canonical.to_string_lossy();
        if let Some(stripped) = text.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(stripped));
        }
    }

    Ok(canonical)
}

fn normalize_path_for_compare(path: &Path) -> String {
    normalize_canonical_path(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string()
}

fn escape_toml_basic_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

struct GitWatchPaths {
    index_path: PathBuf,
    head_path: PathBuf,
}

fn resolve_git_watch_paths(cwd: &Path) -> Result<GitWatchPaths, String> {
    let dot_git = cwd.join(".git");
    let git_dir = if dot_git.is_file() {
        resolve_gitdir_file(cwd, &dot_git)?
    } else {
        dot_git
    };

    Ok(GitWatchPaths {
        index_path: git_dir.join("index"),
        head_path: git_dir.join("HEAD"),
    })
}

fn resolve_gitdir_file(cwd: &Path, dot_git: &Path) -> Result<PathBuf, String> {
    let content = std::fs::read_to_string(dot_git)
        .map_err(|e| format!("Failed to read {}: {}", dot_git.display(), e))?;
    let gitdir = content
        .lines()
        .find_map(|line| line.trim().strip_prefix("gitdir:"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Invalid gitdir file: {}", dot_git.display()))?;

    let path = PathBuf::from(gitdir);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(cwd.join(path))
    }
}

/// Start watching a git repo's index and HEAD for changes.
/// Emits `git-changed` (payload: cwd string) to the frontend on any change,
/// debounced to 150 ms so rapid multi-file operations fire a single event.
/// Replaces any existing watcher for the same path.
#[tauri::command]
pub async fn git_watch(
    cwd: String,
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let GitWatchPaths {
        index_path,
        head_path,
    } = resolve_git_watch_paths(std::path::Path::new(&cwd))?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = tx.send(());
        }
    })
    .map_err(|e| e.to_string())?;

    // Only watch files that exist (non-git dirs won't have these)
    if index_path.exists() {
        watcher
            .watch(&index_path, notify::RecursiveMode::NonRecursive)
            .map_err(|e| e.to_string())?;
    }
    if head_path.exists() {
        watcher
            .watch(&head_path, notify::RecursiveMode::NonRecursive)
            .map_err(|e| e.to_string())?;
    }

    // Store watcher so it stays alive; dropping the old one automatically stops it
    state.git_watchers.lock().await.insert(cwd.clone(), watcher);

    // Debounce task: wait for first event, drain rapid follow-ups within 150 ms, then emit
    let app_handle = app.clone();
    tokio::spawn(async move {
        loop {
            if rx.recv().await.is_none() {
                break; // watcher dropped (git_unwatch called or new watcher replaced this one)
            }
            loop {
                match tokio::time::timeout(std::time::Duration::from_millis(150), rx.recv()).await {
                    Ok(Some(_)) => continue, // more events within window, reset timer
                    Ok(None) => return,      // watcher dropped during debounce
                    Err(_) => break,         // window elapsed, fire
                }
            }
            let _ = app_handle.emit("git-changed", &cwd);
        }
    });

    Ok(())
}

/// Stop watching a git repo path. Safe to call even if no watcher exists.
#[tauri::command]
pub async fn git_unwatch(cwd: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.git_watchers.lock().await.remove(&cwd);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status_simple() {
        let raw = "## main...origin/main [ahead 2, behind 1]\n M src/foo.rs\nA  src/bar.rs\n?? new_file.txt\n";
        let result = parse_porcelain_status(raw);
        assert_eq!(result.branch, "main");
        assert_eq!(result.ahead, 2);
        assert_eq!(result.behind, 1);
        // Should have: staged A for bar.rs, unstaged M for foo.rs, untracked for new_file.txt
        assert_eq!(result.files.len(), 3);
    }

    #[test]
    fn parse_status_no_tracking() {
        let raw = "## feature-branch\nMM both.rs\n";
        let result = parse_porcelain_status(raw);
        assert_eq!(result.branch, "feature-branch");
        assert!(!result.has_upstream);
        assert_eq!(result.upstream, None);
        assert_eq!(result.ahead, 0);
        assert_eq!(result.behind, 0);
        // MM = staged M + unstaged M
        assert_eq!(result.files.len(), 2);
        assert!(result.files[0].is_staged);
        assert!(!result.files[1].is_staged);
    }

    #[test]
    fn parse_status_tracks_upstream_metadata() {
        let raw = "## feature...origin/feature [ahead 2, behind 1]\n";
        let result = parse_porcelain_status(raw);

        assert_eq!(result.branch, "feature");
        assert!(result.has_upstream);
        assert_eq!(result.upstream.as_deref(), Some("origin/feature"));
        assert_eq!(result.ahead, 2);
        assert_eq!(result.behind, 1);
    }

    #[test]
    fn parse_status_rename_uses_new_path() {
        let raw = "## main\nR  old/path.rs -> new/path.rs\n";
        let result = parse_porcelain_status(raw);
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].path, "new/path.rs");
        assert!(result.files[0].is_staged);
    }

    #[test]
    fn run_git_captures_stdout() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        let output = run_git(cwd, &["status", "--porcelain=v1", "-b"]).unwrap();

        assert!(
            output.contains("## "),
            "expected porcelain status on stdout, got: {output:?}"
        );
    }

    #[test]
    fn run_git_reports_missing_program_when_no_candidates_are_available() {
        let temp = tempfile::tempdir().unwrap();
        let error = run_git_with_candidates(temp.path().to_str().unwrap(), &["--version"], &[])
            .unwrap_err();

        assert!(error.contains("Failed to execute git"));
    }

    #[cfg(unix)]
    #[test]
    fn run_git_tries_next_candidate_when_lookup_is_missing() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let fake_git = temp.path().join("git");
        std::fs::write(&fake_git, "#!/bin/sh\necho git version fake\n").unwrap();

        let mut permissions = std::fs::metadata(&fake_git).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_git, permissions).unwrap();

        let missing_git = temp.path().join("missing").join("git");
        let output = run_git_with_candidates(
            temp.path().to_str().unwrap(),
            &["--version"],
            &[missing_git, fake_git],
        )
        .unwrap();

        assert_eq!(output.trim(), "git version fake");
    }

    #[test]
    fn create_worktree_with_build_caches_creates_branch_and_cache_redirects() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let worktree = temp.path().join("agents").join("agent-1").join("worktree");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(
            workspace.join("Cargo.toml"),
            "[package]\nname = \"sample\"\n",
        )
        .unwrap();

        let cwd = workspace.to_str().unwrap();
        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        run_git(cwd, &["add", "Cargo.toml"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        create_worktree_with_build_caches(&workspace, &worktree, "wardian/repo-agent").unwrap();

        assert!(worktree.join(".git").exists());
        assert_eq!(
            run_git(worktree.to_str().unwrap(), &["branch", "--show-current"])
                .unwrap()
                .trim(),
            "wardian/repo-agent"
        );
        assert!(worktree.join(".cargo").join("config.toml").exists());
    }

    #[test]
    fn parse_git_worktree_list_porcelain_extracts_paths_and_branches() {
        let raw = "\
worktree /repo
HEAD 1111111111111111111111111111111111111111
branch refs/heads/main

worktree /repo/.wardian/agents/agent-1/worktrees/review
HEAD 2222222222222222222222222222222222222222
branch refs/heads/feat/review

";

        let worktrees = parse_git_worktree_list_porcelain(raw);

        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0].path, std::path::PathBuf::from("/repo"));
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
        assert_eq!(
            worktrees[1].path,
            std::path::PathBuf::from("/repo/.wardian/agents/agent-1/worktrees/review")
        );
        assert_eq!(worktrees[1].branch.as_deref(), Some("feat/review"));
    }

    #[test]
    fn parse_git_worktree_list_porcelain_handles_detached_or_bare_entries() {
        let raw = "\
worktree /repo-detached
HEAD 3333333333333333333333333333333333333333
detached

worktree /repo-bare
bare

";

        let worktrees = parse_git_worktree_list_porcelain(raw);

        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0].branch, None);
        assert_eq!(worktrees[1].branch, None);
    }

    #[test]
    fn resolve_git_watch_paths_uses_git_dir() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let git_dir = repo.join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();

        let paths = resolve_git_watch_paths(&repo).unwrap();

        assert_eq!(paths.index_path, git_dir.join("index"));
        assert_eq!(paths.head_path, git_dir.join("HEAD"));
    }

    #[test]
    fn resolve_git_watch_paths_uses_linked_worktree_gitdir_file() {
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        let git_dir = temp
            .path()
            .join("main")
            .join(".git")
            .join("worktrees")
            .join("agent");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(
            worktree.join(".git"),
            "gitdir: ../main/.git/worktrees/agent\n",
        )
        .unwrap();

        let paths = resolve_git_watch_paths(&worktree).unwrap();

        assert_eq!(
            paths.index_path,
            worktree.join("../main/.git/worktrees/agent/index")
        );
        assert_eq!(
            paths.head_path,
            worktree.join("../main/.git/worktrees/agent/HEAD")
        );
    }

    #[test]
    fn git_failure_message_never_returns_empty_error() {
        assert_eq!(
            git_failure_message(Some(1), "", ""),
            "git exited with status 1"
        );
    }

    #[tokio::test]
    async fn git_push_publishes_branch_without_upstream() {
        let temp = tempfile::tempdir().unwrap();
        let remote = temp.path().join("remote.git");
        let repo = temp.path().join("repo");

        run_git(
            temp.path().to_str().unwrap(),
            &["init", "--bare", "remote.git"],
        )
        .unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        let cwd = repo.to_str().unwrap();
        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(repo.join("README.md"), "initial\n").unwrap();
        run_git(cwd, &["add", "README.md"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();
        run_git(cwd, &["branch", "-M", "main"]).unwrap();
        run_git(cwd, &["remote", "add", "origin", remote.to_str().unwrap()]).unwrap();
        run_git(cwd, &["push", "-u", "origin", "main"]).unwrap();
        run_git(cwd, &["switch", "-c", "feature/unpublished"]).unwrap();
        std::fs::write(repo.join("feature.txt"), "feature\n").unwrap();
        run_git(cwd, &["add", "feature.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "feature"]).unwrap();

        git_push(cwd.to_string()).await.unwrap();

        let upstream = run_git(
            cwd,
            &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        )
        .unwrap();
        assert_eq!(upstream.trim(), "origin/feature/unpublished");
    }

    #[tokio::test]
    async fn git_diff_file_shows_untracked_file_content() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        std::fs::write(temp.path().join("new.txt"), "first line\nsecond line\n").unwrap();

        let diff = git_diff_file(cwd.to_string(), "new.txt".to_string(), false)
            .await
            .unwrap();

        assert!(diff.contains("+++ b/new.txt"), "{diff}");
        assert!(diff.contains("+first line"), "{diff}");
        assert!(diff.contains("+second line"), "{diff}");
    }

    #[test]
    fn setup_worktree_build_caches_writes_cargo_target_dir_to_workspace_target() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let worktree = temp.path().join("worktree");

        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            workspace.join("Cargo.toml"),
            "[package]\nname = \"sample\"\n",
        )
        .unwrap();

        setup_worktree_build_caches(&worktree, &workspace).unwrap();

        let cargo_config =
            std::fs::read_to_string(worktree.join(".cargo").join("config.toml")).unwrap();
        let expected_target = absolute_existing_path(&workspace)
            .unwrap()
            .join("target")
            .to_string_lossy()
            .replace('\\', "\\\\");
        assert_eq!(
            cargo_config,
            format!("[build]\ntarget-dir = \"{expected_target}\"\n")
        );
    }

    #[test]
    fn setup_worktree_build_caches_links_node_modules_and_existing_python_venv() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let worktree = temp.path().join("worktree");
        let node_modules = workspace.join("node_modules");
        let venv = workspace.join(".venv");

        std::fs::create_dir_all(&node_modules).unwrap();
        std::fs::create_dir_all(&venv).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(workspace.join("package.json"), "{}\n").unwrap();
        std::fs::write(
            workspace.join("pyproject.toml"),
            "[project]\nname = \"sample\"\n",
        )
        .unwrap();

        setup_worktree_build_caches(&worktree, &workspace).unwrap();

        assert_eq!(
            normalize_canonical_path(&worktree.join("node_modules")).unwrap(),
            normalize_canonical_path(&node_modules).unwrap()
        );
        assert_eq!(
            normalize_canonical_path(&worktree.join(".venv")).unwrap(),
            normalize_canonical_path(&venv).unwrap()
        );
    }

    #[test]
    fn setup_worktree_build_caches_skips_python_venv_when_workspace_venv_is_missing() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let worktree = temp.path().join("worktree");

        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(workspace.join("requirements.txt"), "pytest\n").unwrap();

        setup_worktree_build_caches(&worktree, &workspace).unwrap();

        assert!(!worktree.join(".venv").exists());
    }

    #[test]
    fn setup_worktree_build_caches_noops_for_unrecognized_project_type() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let worktree = temp.path().join("worktree");

        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();

        setup_worktree_build_caches(&worktree, &workspace).unwrap();

        assert!(!worktree.join(".cargo").exists());
        assert!(!worktree.join("node_modules").exists());
        assert!(!worktree.join(".venv").exists());
    }
}
