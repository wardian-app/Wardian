use crate::state::AppState;
use crate::utils::fs::create_directory_link;
use crate::utils::process::apply_silent_std_command_policy;
use notify::Watcher;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tauri::{AppHandle, Emitter};
use wardian_core::models::git::{
    GitBranchSummary, GitCommitChangeEntry, GitFileEntry, GitLogEntry, GitStashEntry,
    GitStatusResult,
};

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

fn run_git_with_stdin(cwd: &str, args: &[&str], stdin: &str) -> Result<String, String> {
    let mut last_not_found = None;

    for candidate in git_command_candidates() {
        let mut command = build_git_command(&candidate, cwd, args);
        command.stdin(Stdio::piped());

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                last_not_found = Some(error);
                continue;
            }
            Err(error) => return Err(format!("Failed to execute git: {}", error)),
        };

        if let Some(mut child_stdin) = child.stdin.take() {
            child_stdin
                .write_all(stdin.as_bytes())
                .map_err(|error| format!("Failed to write git input: {}", error))?;
        }

        let output = child
            .wait_with_output()
            .map_err(|error| format!("Failed to execute git: {}", error))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(git_failure_message(
                output.status.code(),
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

    apply_silent_std_command_policy(&mut command);

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
        rebase_in_progress: false,
    }
}

fn git_metadata_path(cwd: &str, path: &str) -> Result<PathBuf, String> {
    let raw_path = run_git(cwd, &["rev-parse", "--git-path", path])?;
    let path = PathBuf::from(raw_path.trim());
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(Path::new(cwd).join(path))
    }
}

fn is_rebase_in_progress(cwd: &str) -> Result<bool, String> {
    Ok(git_metadata_path(cwd, "rebase-merge")?.exists()
        || git_metadata_path(cwd, "rebase-apply")?.exists())
}

#[tauri::command]
pub async fn git_status(cwd: String) -> Result<GitStatusResult, String> {
    let raw = run_git(
        &cwd,
        &["status", "--porcelain=v1", "-b", "--untracked-files=all"],
    )?;
    let mut status = parse_porcelain_status(&raw);
    status.rebase_in_progress = is_rebase_in_progress(&cwd)?;
    Ok(status)
}

#[tauri::command]
pub async fn git_init(cwd: String) -> Result<(), String> {
    run_git(&cwd, &["init"])?;
    Ok(())
}

#[tauri::command]
pub async fn git_clone_repository(cwd: String, repository: String) -> Result<(), String> {
    let repository = repository.trim();
    if repository.is_empty() {
        return Err("Repository URL is required.".to_string());
    }

    run_git(&cwd, &["clone", "--", repository, "."])?;
    Ok(())
}

#[tauri::command]
pub async fn git_current_branch(cwd: String) -> Result<String, String> {
    let branch = run_git(&cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    Ok(branch.trim().to_string())
}

fn build_git_log_args(count: u32, revision: Option<&str>, all: bool) -> Result<Vec<String>, String> {
    let count_str = format!("-{}", count);
    let mut args = vec![
        "log".to_string(),
        count_str,
        "--pretty=format:%H%x1f%P%x1f%D%x1f%s%x1f%an%x1f%ai%x1e".to_string(),
        "--no-color".to_string(),
    ];

    if all {
        args.push("--all".to_string());
        return Ok(args);
    }

    if let Some(revision) = revision.map(str::trim).filter(|revision| !revision.is_empty()) {
        if revision.starts_with('-') {
            return Err("History revision must be a branch, tag, or commit.".to_string());
        }
        args.push(revision.to_string());
        args.push("--".to_string());
    }

    Ok(args)
}

#[tauri::command]
pub async fn git_log(
    cwd: String,
    count: u32,
    revision: Option<String>,
    all: Option<bool>,
) -> Result<Vec<GitLogEntry>, String> {
    let args = build_git_log_args(count, revision.as_deref(), all.unwrap_or(false))?;
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let raw = run_git(&cwd, &arg_refs)?;

    Ok(parse_git_log_entries(&raw))
}

fn parse_git_ref_names(raw: &str) -> Vec<String> {
    let mut refs = Vec::new();
    for part in raw
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        if let Some(target) = part.strip_prefix("HEAD -> ") {
            refs.push("HEAD".to_string());
            refs.push(target.trim().to_string());
            continue;
        }
        if let Some(tag) = part.strip_prefix("tag: ") {
            refs.push(tag.trim().to_string());
            continue;
        }
        refs.push(part.to_string());
    }
    refs.dedup();
    refs
}

fn parse_git_log_entries(raw: &str) -> Vec<GitLogEntry> {
    raw.split('\x1e')
        .filter_map(|record| {
            let record = record.trim_matches(['\r', '\n']);
            if record.is_empty() {
                return None;
            }

            let fields: Vec<&str> = record.split('\x1f').collect();
            if fields.len() != 6 {
                return None;
            }

            Some(GitLogEntry {
                hash: fields[0].to_string(),
                parent_hashes: fields[1]
                    .split_whitespace()
                    .map(ToString::to_string)
                    .collect(),
                refs: parse_git_ref_names(fields[2]),
                message: fields[3].to_string(),
                author: fields[4].to_string(),
                date: fields[5].to_string(),
            })
        })
        .collect()
}

fn parse_git_branches(raw: &str) -> Vec<GitBranchSummary> {
    raw.lines()
        .filter_map(|line| {
            let head = line.chars().next()?;
            let name = line.get(2..)?.trim();
            if name.is_empty() {
                return None;
            }
            Some(GitBranchSummary {
                name: name.to_string(),
                current: head == '*',
            })
        })
        .collect()
}

fn parse_git_stashes(raw: &str) -> Vec<GitStashEntry> {
    let fields: Vec<&str> = raw
        .split('\0')
        .map(|part| part.trim_matches(['\r', '\n']))
        .filter(|part| !part.is_empty())
        .collect();

    fields
        .chunks(2)
        .filter_map(|chunk| {
            let selector = chunk.first()?.trim();
            let message = chunk.get(1).map(|value| value.trim()).unwrap_or_default();
            if selector.is_empty() {
                return None;
            }
            Some(GitStashEntry {
                selector: selector.to_string(),
                message: message.to_string(),
            })
        })
        .collect()
}

fn parse_git_commit_changes(raw: &str) -> Vec<GitCommitChangeEntry> {
    raw.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let status = parts.next()?.trim();
            if status.is_empty() {
                return None;
            }

            let path = if status.starts_with('R') || status.starts_with('C') {
                parts.next_back()?
            } else {
                parts.next()?
            };

            Some(GitCommitChangeEntry {
                path: parse_porcelain_path(path),
                status: status
                    .chars()
                    .next()
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| status.to_string()),
            })
        })
        .collect()
}

#[tauri::command]
pub async fn git_commit_changes(
    cwd: String,
    hash: String,
    parent_hash: Option<String>,
) -> Result<Vec<GitCommitChangeEntry>, String> {
    let raw = if let Some(parent_hash) = parent_hash.filter(|value| !value.trim().is_empty()) {
        run_git(
            &cwd,
            &[
                "diff",
                "--name-status",
                "--find-renames",
                "--no-color",
                parent_hash.trim(),
                hash.trim(),
            ],
        )?
    } else {
        run_git(
            &cwd,
            &[
                "diff-tree",
                "--root",
                "--no-commit-id",
                "--name-status",
                "-r",
                "--find-renames",
                "--no-color",
                hash.trim(),
            ],
        )?
    };

    Ok(parse_git_commit_changes(&raw))
}

#[tauri::command]
pub async fn git_commit_diff(
    cwd: String,
    hash: String,
    parent_hash: Option<String>,
) -> Result<String, String> {
    if let Some(parent_hash) = parent_hash.filter(|value| !value.trim().is_empty()) {
        return run_git(
            &cwd,
            &[
                "diff",
                "--no-color",
                "--find-renames",
                parent_hash.trim(),
                hash.trim(),
            ],
        );
    }

    run_git(
        &cwd,
        &[
            "show",
            "--format=",
            "--patch",
            "--no-color",
            "--find-renames",
            hash.trim(),
        ],
    )
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
pub async fn git_diff_file_against_workspace(cwd: String, path: String) -> Result<String, String> {
    run_git(&cwd, &["diff", "--no-color", "--", &path])
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
pub async fn git_apply_diff_hunk(
    cwd: String,
    patch: String,
    reverse: bool,
) -> Result<(), String> {
    if patch.trim().is_empty() {
        return Err("Diff hunk patch is required.".to_string());
    }

    let args: Vec<&str> = if reverse {
        vec!["apply", "--cached", "--reverse"]
    } else {
        vec!["apply", "--cached"]
    };
    run_git_with_stdin(&cwd, &args, &patch)?;
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

fn gitignore_pattern(path: &str) -> Result<String, String> {
    let normalized = path.trim().replace('\\', "/");
    let trimmed = normalized.trim_start_matches("./");
    if trimmed.is_empty() || trimmed.starts_with('/') || trimmed.split('/').any(|part| part == "..")
    {
        return Err("Only repository-relative paths can be ignored.".to_string());
    }
    Ok(trimmed.to_string())
}

fn git_revision_path(path: &str) -> Result<String, String> {
    let normalized = path.trim().replace('\\', "/");
    let trimmed = normalized.trim_start_matches("./");
    if trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.contains(':')
        || trimmed.split('/').any(|part| part == "..")
    {
        return Err("Only repository-relative paths can be opened from a revision.".to_string());
    }
    Ok(trimmed.to_string())
}

fn git_revision_name(revision: &str) -> Result<String, String> {
    let trimmed = revision.trim();
    if trimmed.is_empty() || trimmed.contains(':') || trimmed.contains("..") {
        return Err("Invalid git revision.".to_string());
    }
    Ok(trimmed.to_string())
}

#[tauri::command]
pub async fn git_show_file_revision(
    cwd: String,
    path: String,
    revision: String,
) -> Result<String, String> {
    let path = git_revision_path(&path)?;
    let revision = git_revision_name(&revision)?;
    let spec = format!("{revision}:{path}");
    run_git(&cwd, &["show", "--no-ext-diff", "--no-color", &spec])
}

#[tauri::command]
pub async fn git_ignore(cwd: String, paths: Vec<String>) -> Result<(), String> {
    if paths.is_empty() {
        return Err("At least one path is required.".to_string());
    }

    let gitignore_path = Path::new(&cwd).join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
    let mut existing_patterns: std::collections::HashSet<String> = existing
        .lines()
        .map(|line| line.trim().to_string())
        .collect();
    let mut additions = Vec::new();

    for path in paths {
        let pattern = gitignore_pattern(&path)?;
        if existing_patterns.insert(pattern.clone()) {
            additions.push(pattern);
        }
    }

    if additions.is_empty() {
        return Ok(());
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    for addition in additions {
        updated.push_str(&addition);
        updated.push('\n');
    }
    std::fs::write(gitignore_path, updated).map_err(|err| err.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn git_commit(cwd: String, message: String) -> Result<(), String> {
    run_git(&cwd, &["commit", "-m", &message])?;
    Ok(())
}

#[tauri::command]
pub async fn git_commit_signed(cwd: String, message: String) -> Result<(), String> {
    run_git(&cwd, &["commit", "--signoff", "-m", &message])?;
    Ok(())
}

#[tauri::command]
pub async fn git_commit_staged_signed(cwd: String, message: String) -> Result<(), String> {
    run_git(&cwd, &["commit", "--signoff", "-m", &message])?;
    Ok(())
}

#[tauri::command]
pub async fn git_commit_all_signed(cwd: String, message: String) -> Result<(), String> {
    run_git(&cwd, &["add", "--all"])?;
    run_git(&cwd, &["commit", "--signoff", "-m", &message])?;
    Ok(())
}

#[tauri::command]
pub async fn git_commit_signed_no_verify(cwd: String, message: String) -> Result<(), String> {
    run_git(&cwd, &["commit", "--signoff", "--no-verify", "-m", &message])?;
    Ok(())
}

#[tauri::command]
pub async fn git_commit_staged_signed_no_verify(
    cwd: String,
    message: String,
) -> Result<(), String> {
    run_git(&cwd, &["commit", "--signoff", "--no-verify", "-m", &message])?;
    Ok(())
}

#[tauri::command]
pub async fn git_commit_all_signed_no_verify(cwd: String, message: String) -> Result<(), String> {
    run_git(&cwd, &["add", "--all"])?;
    run_git(&cwd, &["commit", "--signoff", "--no-verify", "-m", &message])?;
    Ok(())
}

#[tauri::command]
pub async fn git_rebase_abort(cwd: String) -> Result<(), String> {
    run_git(&cwd, &["rebase", "--abort"])?;
    Ok(())
}

#[tauri::command]
pub async fn git_commit_no_verify(cwd: String, message: String) -> Result<(), String> {
    run_git(&cwd, &["commit", "--no-verify", "-m", &message])?;
    Ok(())
}

#[tauri::command]
pub async fn git_commit_staged_no_verify(cwd: String, message: String) -> Result<(), String> {
    run_git(&cwd, &["commit", "--no-verify", "-m", &message])?;
    Ok(())
}

#[tauri::command]
pub async fn git_commit_all_no_verify(cwd: String, message: String) -> Result<(), String> {
    run_git(&cwd, &["add", "--all"])?;
    run_git(&cwd, &["commit", "--no-verify", "-m", &message])?;
    Ok(())
}

#[tauri::command]
pub async fn git_commit_empty(cwd: String, message: String) -> Result<(), String> {
    run_git(&cwd, &["commit", "--allow-empty", "-m", &message])?;
    Ok(())
}

#[tauri::command]
pub async fn git_commit_empty_no_verify(cwd: String, message: String) -> Result<(), String> {
    run_git(
        &cwd,
        &["commit", "--allow-empty", "--no-verify", "-m", &message],
    )?;
    Ok(())
}

#[tauri::command]
pub async fn git_commit_amend(cwd: String, message: String) -> Result<(), String> {
    run_git(&cwd, &["commit", "--amend", "-m", &message])?;
    Ok(())
}

#[tauri::command]
pub async fn git_commit_amend_no_verify(cwd: String, message: String) -> Result<(), String> {
    run_git(&cwd, &["commit", "--amend", "--no-verify", "-m", &message])?;
    Ok(())
}

#[tauri::command]
pub async fn git_commit_staged_amend_no_verify(cwd: String, message: String) -> Result<(), String> {
    run_git(&cwd, &["commit", "--amend", "--no-verify", "-m", &message])?;
    Ok(())
}

#[tauri::command]
pub async fn git_commit_all_amend_no_verify(cwd: String, message: String) -> Result<(), String> {
    run_git(&cwd, &["add", "--all"])?;
    run_git(&cwd, &["commit", "--amend", "--no-verify", "-m", &message])?;
    Ok(())
}

#[tauri::command]
pub async fn git_commit_staged_amend(cwd: String, message: String) -> Result<(), String> {
    run_git(&cwd, &["commit", "--amend", "-m", &message])?;
    Ok(())
}

#[tauri::command]
pub async fn git_commit_all_amend(cwd: String, message: String) -> Result<(), String> {
    run_git(&cwd, &["add", "--all"])?;
    run_git(&cwd, &["commit", "--amend", "-m", &message])?;
    Ok(())
}

#[tauri::command]
pub async fn git_undo_last_commit(cwd: String) -> Result<String, String> {
    let message = run_git(&cwd, &["log", "-1", "--pretty=%B"])?
        .trim_end()
        .to_string();
    let parents = run_git(&cwd, &["rev-list", "--parents", "-n", "1", "HEAD"])?;
    let fields: Vec<&str> = parents.split_whitespace().collect();
    if fields.is_empty() {
        return Err("Can't undo because HEAD doesn't point to any commit.".to_string());
    }

    if fields.len() > 1 {
        run_git(&cwd, &["reset", "HEAD~"])?;
    } else {
        run_git(&cwd, &["update-ref", "-d", "HEAD"])?;
        let _ = run_git(&cwd, &["reset"]);
    }

    Ok(message)
}

#[tauri::command]
pub async fn git_pull(cwd: String) -> Result<String, String> {
    run_git(&cwd, &["pull"])
}

#[tauri::command]
pub async fn git_list_branches(cwd: String) -> Result<Vec<GitBranchSummary>, String> {
    let raw = run_git(&cwd, &["branch", "--format=%(HEAD) %(refname:short)"])?;
    Ok(parse_git_branches(&raw))
}

#[tauri::command]
pub async fn git_checkout_branch(cwd: String, branch: String) -> Result<(), String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("Branch name is required.".to_string());
    }
    run_git(&cwd, &["checkout", branch])?;
    Ok(())
}

#[tauri::command]
pub async fn git_create_branch(cwd: String, branch: String) -> Result<(), String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("Branch name is required.".to_string());
    }
    run_git(&cwd, &["check-ref-format", "--branch", branch])?;
    run_git(&cwd, &["checkout", "-b", branch])?;
    Ok(())
}

#[tauri::command]
pub async fn git_stash_push(cwd: String, include_untracked: bool) -> Result<String, String> {
    let mut args = vec!["stash", "push", "-m", "Wardian stash"];
    if include_untracked {
        args.push("--include-untracked");
    }
    run_git(&cwd, &args)
}

#[tauri::command]
pub async fn git_list_stashes(cwd: String) -> Result<Vec<GitStashEntry>, String> {
    let raw = run_git(&cwd, &["stash", "list", "--format=%gd%x00%gs%x00"])?;
    Ok(parse_git_stashes(&raw))
}

#[tauri::command]
pub async fn git_show_stash(cwd: String, stash: String) -> Result<String, String> {
    let stash = stash.trim();
    if stash.is_empty() {
        return Err("Stash selector is required.".to_string());
    }
    run_git(&cwd, &["stash", "show", "--patch", "--no-color", stash])
}

#[tauri::command]
pub async fn git_stash_staged(cwd: String) -> Result<String, String> {
    run_git(
        &cwd,
        &["stash", "push", "--staged", "-m", "Wardian staged stash"],
    )
}

#[tauri::command]
pub async fn git_stash_apply_latest(cwd: String) -> Result<String, String> {
    run_git(&cwd, &["stash", "apply"])
}

#[tauri::command]
pub async fn git_stash_apply(cwd: String, stash: String) -> Result<String, String> {
    let stash = stash.trim();
    if stash.is_empty() {
        return Err("Stash selector is required.".to_string());
    }
    run_git(&cwd, &["stash", "apply", stash])
}

#[tauri::command]
pub async fn git_stash_pop_latest(cwd: String) -> Result<String, String> {
    run_git(&cwd, &["stash", "pop"])
}

#[tauri::command]
pub async fn git_stash_pop(cwd: String, stash: String) -> Result<String, String> {
    let stash = stash.trim();
    if stash.is_empty() {
        return Err("Stash selector is required.".to_string());
    }
    run_git(&cwd, &["stash", "pop", stash])
}

#[tauri::command]
pub async fn git_stash_drop(cwd: String, stash: String) -> Result<String, String> {
    let stash = stash.trim();
    if stash.is_empty() {
        return Err("Stash selector is required.".to_string());
    }
    run_git(&cwd, &["stash", "drop", stash])
}

#[tauri::command]
pub async fn git_stash_drop_all(cwd: String) -> Result<String, String> {
    run_git(&cwd, &["stash", "clear"])
}

#[tauri::command]
pub async fn git_fetch(cwd: String) -> Result<String, String> {
    run_git(&cwd, &["fetch"])
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
    remove_worktree_with_options(workspace_path, worktree_path, true)
}

pub(crate) fn remove_worktree_without_force(
    workspace_path: &Path,
    worktree_path: &Path,
) -> Result<(), String> {
    cleanup_generated_worktree_build_caches(workspace_path, worktree_path)?;
    remove_worktree_with_options(workspace_path, worktree_path, false)
}

fn cleanup_generated_worktree_build_caches(
    workspace_path: &Path,
    worktree_path: &Path,
) -> Result<(), String> {
    let workspace_path = absolute_existing_path(workspace_path)?;
    let worktree_path = absolute_worktree_target_path(&workspace_path, worktree_path);

    if workspace_path.join("Cargo.toml").is_file() {
        remove_generated_cargo_config(&worktree_path, &workspace_path)?;
    }

    if workspace_path.join("package.json").is_file() {
        remove_generated_cache_link(
            &worktree_path.join("node_modules"),
            &workspace_path.join("node_modules"),
        )?;
    }

    if workspace_path.join("pyproject.toml").is_file()
        || workspace_path.join("requirements.txt").is_file()
    {
        remove_generated_cache_link(&worktree_path.join(".venv"), &workspace_path.join(".venv"))?;
    }

    Ok(())
}

fn remove_generated_cargo_config(
    worktree_path: &Path,
    workspace_path: &Path,
) -> Result<(), String> {
    let cargo_dir = worktree_path.join(".cargo");
    let config_path = cargo_dir.join("config.toml");
    if !config_path.is_file() {
        return Ok(());
    }

    let target_dir = workspace_path.join("target");
    let expected = format!(
        "[build]\ntarget-dir = \"{}\"\n",
        escape_toml_basic_string(&target_dir.to_string_lossy())
    );
    let actual = std::fs::read_to_string(&config_path).map_err(|e| e.to_string())?;
    if actual != expected {
        return Ok(());
    }
    if git_tracks_relative_path(worktree_path, ".cargo/config.toml")? {
        return Ok(());
    }

    std::fs::remove_file(&config_path).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir(&cargo_dir);
    Ok(())
}

fn git_tracks_relative_path(repo_path: &Path, relative_path: &str) -> Result<bool, String> {
    let cwd = path_to_git_arg(repo_path)?;
    let raw = run_git(&cwd, &["ls-files", "--", relative_path])?;
    Ok(raw.lines().any(|line| line.trim() == relative_path))
}

fn remove_generated_cache_link(link: &Path, target: &Path) -> Result<(), String> {
    if !(link.exists() || link.symlink_metadata().is_ok()) {
        return Ok(());
    }
    if !target.exists() || !link_matches_target(link, target) {
        return Ok(());
    }

    remove_link_path(link)
}

fn remove_link_path(path: &Path) -> Result<(), String> {
    std::fs::remove_dir(path)
        .or_else(|_| std::fs::remove_file(path))
        .map_err(|e| e.to_string())
}

fn remove_worktree_with_options(
    workspace_path: &Path,
    worktree_path: &Path,
    force: bool,
) -> Result<(), String> {
    let workspace_path = absolute_existing_path(workspace_path)?;
    let worktree_path = absolute_worktree_target_path(&workspace_path, worktree_path);
    let workspace = path_to_git_arg(&workspace_path)?;
    let worktree = path_to_git_arg(&worktree_path)?;
    if force {
        run_git(&workspace, &["worktree", "remove", &worktree, "--force"])?;
    } else {
        run_git(&workspace, &["worktree", "remove", &worktree])?;
    }
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

    Ok(list_git_worktrees(&source_path)?
        .into_iter()
        .any(|entry| normalize_path_for_compare(&entry.path) == expected))
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
    let config_path = cargo_dir.join("config.toml");
    if config_path.exists() || config_path.symlink_metadata().is_ok() {
        return Ok(());
    }

    let target_dir = workspace_path.join("target");
    let config = format!(
        "[build]\ntarget-dir = \"{}\"\n",
        escape_toml_basic_string(&target_dir.to_string_lossy())
    );
    std::fs::write(config_path, config).map_err(|e| e.to_string())
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
    use std::fs;

    fn first_hunk_patch(diff: &str) -> String {
        let mut lines = Vec::new();
        let mut hunk_count = 0;

        for line in diff.lines() {
            if line.starts_with("@@") {
                hunk_count += 1;
                if hunk_count > 1 {
                    break;
                }
            }

            if hunk_count <= 1 {
                lines.push(line);
            }
        }

        format!("{}\n", lines.join("\n"))
    }

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
    fn parse_git_log_entries_preserves_parent_hashes_and_refs() {
        let raw = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\x1fb1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1 c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2\x1fHEAD -> main, origin/main, tag: v1.0\x1fMerge feature branch\x1fAda Lovelace\x1f2026-06-25 08:00:00 -0400\x1e\
dddddddddddddddddddddddddddddddddddddddd\x1f\x1ffeature/review\x1fInitial commit\x1fGrace Hopper\x1f2026-06-24 07:00:00 -0400\x1e";

        let entries = parse_git_log_entries(raw);

        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].parent_hashes,
            vec![
                "b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1".to_string(),
                "c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2".to_string()
            ]
        );
        assert_eq!(
            entries[0].refs,
            vec![
                "HEAD".to_string(),
                "main".to_string(),
                "origin/main".to_string(),
                "v1.0".to_string()
            ]
        );
        assert!(entries[1].parent_hashes.is_empty());
        assert_eq!(entries[1].refs, vec!["feature/review".to_string()]);
    }

    #[test]
    fn build_git_log_args_targets_selected_revision() {
        let args = build_git_log_args(100, Some("origin/main"), false).unwrap();

        assert_eq!(
            args,
            vec![
                "log".to_string(),
                "-100".to_string(),
                "--pretty=format:%H%x1f%P%x1f%D%x1f%s%x1f%an%x1f%ai%x1e".to_string(),
                "--no-color".to_string(),
                "origin/main".to_string(),
                "--".to_string(),
            ]
        );
    }

    #[test]
    fn build_git_log_args_can_target_all_refs() {
        let args = build_git_log_args(50, None, true).unwrap();

        assert_eq!(args.last(), Some(&"--all".to_string()));
    }

    #[test]
    fn build_git_log_args_rejects_option_like_revision() {
        let err = build_git_log_args(50, Some("--all"), false).unwrap_err();

        assert_eq!(err, "History revision must be a branch, tag, or commit.");
    }

    #[test]
    fn parse_git_commit_changes_preserves_status_and_rename_target() {
        let raw =
            "M\tsrc/changed.ts\nA\tsrc/new.ts\nD\tsrc/old.ts\nR100\tsrc/before.ts\tsrc/after.ts\n";

        let changes = parse_git_commit_changes(raw);

        assert_eq!(changes.len(), 4);
        assert_eq!(changes[0].status, "M");
        assert_eq!(changes[0].path, "src/changed.ts");
        assert_eq!(changes[2].status, "D");
        assert_eq!(changes[2].path, "src/old.ts");
        assert_eq!(changes[3].status, "R");
        assert_eq!(changes[3].path, "src/after.ts");
    }

    #[tokio::test]
    async fn git_commit_diff_compares_commit_with_parent() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        fs::write(temp.path().join("tracked.txt"), "old\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();
        let parent = run_git(cwd, &["rev-parse", "HEAD"]).unwrap().trim().to_string();

        fs::write(temp.path().join("tracked.txt"), "new\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "change tracked"]).unwrap();
        let hash = run_git(cwd, &["rev-parse", "HEAD"]).unwrap().trim().to_string();

        let diff = git_commit_diff(cwd.to_string(), hash, Some(parent)).await.unwrap();

        assert!(diff.contains("diff --git a/tracked.txt b/tracked.txt"));
        assert!(diff.contains("-old"));
        assert!(diff.contains("+new"));
    }

    #[test]
    fn parse_git_stashes_preserves_selector_and_message() {
        let raw = "stash@{0}\0WIP on main: second stash\0stash@{1}\0On feature: first stash\0";

        let stashes = parse_git_stashes(raw);

        assert_eq!(stashes.len(), 2);
        assert_eq!(stashes[0].selector, "stash@{0}");
        assert_eq!(stashes[0].message, "WIP on main: second stash");
        assert_eq!(stashes[1].selector, "stash@{1}");
        assert_eq!(stashes[1].message, "On feature: first stash");
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
    fn remove_worktree_without_force_cleans_generated_cache_redirects() {
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
        assert!(worktree.join(".cargo").join("config.toml").exists());

        remove_worktree_without_force(&workspace, &worktree).unwrap();

        assert!(!worktree.exists());
        assert!(!git_worktree_contains_path(&workspace, &worktree).unwrap());
    }

    #[test]
    fn remove_worktree_without_force_preserves_dirty_worktree_failure() {
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
        std::fs::write(worktree.join("local.txt"), "keep me\n").unwrap();

        let err = remove_worktree_without_force(&workspace, &worktree)
            .expect_err("dirty worktree should block non-force removal");

        assert!(
            err.contains("modified or untracked") || err.contains("contains"),
            "{err}"
        );
        assert!(worktree.join("local.txt").exists());
        assert!(git_worktree_contains_path(&workspace, &worktree).unwrap());
    }

    #[test]
    fn remove_worktree_without_force_preserves_tracked_matching_cargo_config_on_failure() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let worktree = temp.path().join("agents").join("agent-1").join("worktree");
        std::fs::create_dir_all(workspace.join(".cargo")).unwrap();
        std::fs::write(
            workspace.join("Cargo.toml"),
            "[package]\nname = \"sample\"\n",
        )
        .unwrap();
        let expected_target = absolute_existing_path(&workspace)
            .unwrap()
            .join("target")
            .to_string_lossy()
            .replace('\\', "\\\\");
        let tracked_config = format!("[build]\ntarget-dir = \"{expected_target}\"\n");
        std::fs::write(
            workspace.join(".cargo").join("config.toml"),
            &tracked_config,
        )
        .unwrap();

        let cwd = workspace.to_str().unwrap();
        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        run_git(cwd, &["config", "core.autocrlf", "false"]).unwrap();
        run_git(cwd, &["add", "Cargo.toml", ".cargo/config.toml"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        create_worktree_with_build_caches(&workspace, &worktree, "wardian/repo-agent").unwrap();
        std::fs::write(worktree.join("local.txt"), "keep me\n").unwrap();

        let err = remove_worktree_without_force(&workspace, &worktree)
            .expect_err("dirty worktree should block non-force removal");

        assert!(
            err.contains("modified or untracked") || err.contains("contains"),
            "{err}"
        );
        assert_eq!(
            std::fs::read_to_string(worktree.join(".cargo").join("config.toml")).unwrap(),
            tracked_config
        );
        assert!(git_worktree_contains_path(&workspace, &worktree).unwrap());
    }

    #[test]
    fn git_worktree_contains_path_detects_created_worktree() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let cwd = repo.to_str().unwrap();
        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(repo.join("README.md"), "initial\n").unwrap();
        run_git(cwd, &["add", "README.md"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        let worktree = temp.path().join("review");
        create_worktree_with_build_caches(&repo, &worktree, "feat/review").unwrap();

        assert!(git_worktree_contains_path(&repo, &worktree).unwrap());
        assert!(!git_worktree_contains_path(&repo, &temp.path().join("not-a-worktree")).unwrap());
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
    async fn git_fetch_updates_remote_tracking_refs_without_merging() {
        let temp = tempfile::tempdir().unwrap();
        let remote = temp.path().join("remote.git");
        let repo = temp.path().join("repo");
        let peer = temp.path().join("peer");

        run_git(
            temp.path().to_str().unwrap(),
            &["init", "--bare", "remote.git"],
        )
        .unwrap();
        run_git(
            remote.to_str().unwrap(),
            &["symbolic-ref", "HEAD", "refs/heads/main"],
        )
        .unwrap();
        run_git(
            temp.path().to_str().unwrap(),
            &["clone", remote.to_str().unwrap(), repo.to_str().unwrap()],
        )
        .unwrap();

        let cwd = repo.to_str().unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(repo.join("README.md"), "initial\n").unwrap();
        run_git(cwd, &["add", "README.md"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();
        run_git(cwd, &["branch", "-M", "main"]).unwrap();
        run_git(cwd, &["push", "-u", "origin", "main"]).unwrap();

        run_git(
            temp.path().to_str().unwrap(),
            &["clone", remote.to_str().unwrap(), peer.to_str().unwrap()],
        )
        .unwrap();
        let peer_cwd = peer.to_str().unwrap();
        run_git(peer_cwd, &["checkout", "main"]).unwrap();
        run_git(peer_cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(peer_cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(peer.join("remote.txt"), "remote\n").unwrap();
        run_git(peer_cwd, &["add", "remote.txt"]).unwrap();
        run_git(peer_cwd, &["commit", "-m", "remote change"]).unwrap();
        run_git(peer_cwd, &["push", "origin", "main"]).unwrap();

        let local_head_before = run_git(cwd, &["rev-parse", "HEAD"]).unwrap();
        let remote_head = run_git(peer_cwd, &["rev-parse", "HEAD"]).unwrap();

        git_fetch(cwd.to_string()).await.unwrap();

        let fetched_origin = run_git(cwd, &["rev-parse", "refs/remotes/origin/main"]).unwrap();
        let local_head_after = run_git(cwd, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(fetched_origin.trim(), remote_head.trim());
        assert_eq!(local_head_after.trim(), local_head_before.trim());
    }

    #[tokio::test]
    async fn git_checkout_branch_lists_and_switches_local_branches() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("README.md"), "initial\n").unwrap();
        run_git(cwd, &["add", "README.md"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();
        run_git(cwd, &["branch", "-M", "main"]).unwrap();
        run_git(cwd, &["branch", "feature/source-control"]).unwrap();

        let branches = git_list_branches(cwd.to_string()).await.unwrap();
        assert_eq!(branches.len(), 2);
        assert!(branches
            .iter()
            .any(|branch| branch.name == "main" && branch.current));
        assert!(branches
            .iter()
            .any(|branch| branch.name == "feature/source-control" && !branch.current));

        git_checkout_branch(cwd.to_string(), "feature/source-control".to_string())
            .await
            .unwrap();

        let current = run_git(cwd, &["branch", "--show-current"]).unwrap();
        assert_eq!(current.trim(), "feature/source-control");
    }

    #[tokio::test]
    async fn git_create_branch_creates_and_checks_out_local_branch() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("README.md"), "initial\n").unwrap();
        run_git(cwd, &["add", "README.md"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();
        run_git(cwd, &["branch", "-M", "main"]).unwrap();

        git_create_branch(cwd.to_string(), "feature/new-branch".to_string())
            .await
            .unwrap();

        let current = run_git(cwd, &["branch", "--show-current"]).unwrap();
        assert_eq!(current.trim(), "feature/new-branch");
        let branch_ref = run_git(cwd, &["rev-parse", "--verify", "feature/new-branch"]).unwrap();
        let head = run_git(cwd, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(branch_ref.trim(), head.trim());
    }

    #[tokio::test]
    async fn git_stash_push_include_untracked_and_pop_latest_round_trips_worktree() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "initial\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "changed\n").unwrap();
        std::fs::write(temp.path().join("new.txt"), "untracked\n").unwrap();

        git_stash_push(cwd.to_string(), true).await.unwrap();

        let status_after_stash = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status_after_stash.trim().is_empty(), "{status_after_stash}");
        let stash_list = run_git(cwd, &["stash", "list"]).unwrap();
        assert!(stash_list.contains("Wardian stash"), "{stash_list}");

        git_stash_pop_latest(cwd.to_string()).await.unwrap();

        let status_after_pop = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(
            status_after_pop.contains(" M tracked.txt"),
            "{status_after_pop}"
        );
        assert!(
            status_after_pop.contains("?? new.txt"),
            "{status_after_pop}"
        );
    }

    #[tokio::test]
    async fn git_stash_apply_latest_restores_worktree_and_keeps_stash() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "initial\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "changed\n").unwrap();

        git_stash_push(cwd.to_string(), false).await.unwrap();
        git_stash_apply_latest(cwd.to_string()).await.unwrap();

        let status_after_apply = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(
            status_after_apply.contains(" M tracked.txt"),
            "{status_after_apply}"
        );
        let stash_list = run_git(cwd, &["stash", "list"]).unwrap();
        assert!(stash_list.contains("Wardian stash"), "{stash_list}");
    }

    #[tokio::test]
    async fn git_stash_apply_applies_selected_stash_and_keeps_stash() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "initial\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        std::fs::write(temp.path().join("tracked.txt"), "first stash\n").unwrap();
        git_stash_push(cwd.to_string(), false).await.unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "second stash\n").unwrap();
        git_stash_push(cwd.to_string(), false).await.unwrap();

        git_stash_apply(cwd.to_string(), "stash@{1}".to_string())
            .await
            .unwrap();

        let content = std::fs::read_to_string(temp.path().join("tracked.txt"))
            .unwrap()
            .replace("\r\n", "\n");
        assert_eq!(content, "first stash\n");
        let stash_list = run_git(cwd, &["stash", "list"]).unwrap();
        assert_eq!(stash_list.lines().count(), 2, "{stash_list}");
    }

    #[tokio::test]
    async fn git_stash_pop_applies_selected_stash_and_removes_it() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "initial\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        std::fs::write(temp.path().join("tracked.txt"), "first stash\n").unwrap();
        git_stash_push(cwd.to_string(), false).await.unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "second stash\n").unwrap();
        git_stash_push(cwd.to_string(), false).await.unwrap();

        git_stash_pop(cwd.to_string(), "stash@{1}".to_string())
            .await
            .unwrap();

        let content = std::fs::read_to_string(temp.path().join("tracked.txt"))
            .unwrap()
            .replace("\r\n", "\n");
        assert_eq!(content, "first stash\n");
        let stash_list = run_git(cwd, &["stash", "list"]).unwrap();
        assert_eq!(stash_list.lines().count(), 1, "{stash_list}");
        assert!(!stash_list.contains("stash@{1}"), "{stash_list}");
    }

    #[tokio::test]
    async fn git_stash_staged_stashes_only_staged_changes() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("index_only.txt"), "initial staged\n").unwrap();
        std::fs::write(temp.path().join("worktree_only.txt"), "initial unstaged\n").unwrap();
        run_git(cwd, &["add", "index_only.txt", "worktree_only.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        std::fs::write(temp.path().join("index_only.txt"), "changed staged\n").unwrap();
        std::fs::write(temp.path().join("worktree_only.txt"), "changed unstaged\n").unwrap();
        run_git(cwd, &["add", "index_only.txt"]).unwrap();

        git_stash_staged(cwd.to_string()).await.unwrap();

        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(!status.contains("index_only.txt"), "{status}");
        assert!(status.contains(" M worktree_only.txt"), "{status}");
        let stash_list = run_git(cwd, &["stash", "list"]).unwrap();
        assert!(stash_list.contains("Wardian staged stash"), "{stash_list}");
    }

    #[tokio::test]
    async fn git_stash_drop_all_clears_stash_entries() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "initial\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        std::fs::write(temp.path().join("tracked.txt"), "first stash\n").unwrap();
        git_stash_push(cwd.to_string(), false).await.unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "second stash\n").unwrap();
        git_stash_push(cwd.to_string(), false).await.unwrap();

        let stash_list_before = run_git(cwd, &["stash", "list"]).unwrap();
        assert_eq!(stash_list_before.lines().count(), 2, "{stash_list_before}");

        git_stash_drop_all(cwd.to_string()).await.unwrap();

        let stash_list_after = run_git(cwd, &["stash", "list"]).unwrap();
        assert!(stash_list_after.trim().is_empty(), "{stash_list_after}");
    }

    #[tokio::test]
    async fn git_stash_drop_removes_selected_stash_only() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "initial\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        std::fs::write(temp.path().join("tracked.txt"), "first stash\n").unwrap();
        run_git(cwd, &["stash", "push", "-m", "first stash"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "second stash\n").unwrap();
        run_git(cwd, &["stash", "push", "-m", "second stash"]).unwrap();

        git_stash_drop(cwd.to_string(), "stash@{1}".to_string())
            .await
            .unwrap();

        let stash_list = run_git(cwd, &["stash", "list"]).unwrap();
        assert_eq!(stash_list.lines().count(), 1, "{stash_list}");
        assert!(stash_list.contains("second stash"), "{stash_list}");
        assert!(!stash_list.contains("first stash"), "{stash_list}");
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status.trim().is_empty(), "{status}");
    }

    #[tokio::test]
    async fn git_list_and_show_stashes_returns_selected_stash_diff() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "initial\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "stash preview\n").unwrap();
        git_stash_push(cwd.to_string(), false).await.unwrap();

        let stashes = git_list_stashes(cwd.to_string()).await.unwrap();
        assert_eq!(stashes.len(), 1);
        assert_eq!(stashes[0].selector, "stash@{0}");
        assert!(stashes[0].message.contains("Wardian stash"), "{stashes:?}");

        let diff = git_show_stash(cwd.to_string(), stashes[0].selector.clone())
            .await
            .unwrap();
        assert!(diff.contains("diff --git"), "{diff}");
        assert!(diff.contains("+stash preview"), "{diff}");
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

    #[tokio::test]
    async fn git_apply_diff_hunk_stages_only_the_selected_worktree_hunk() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        let base = (1..=24)
            .map(|line| format!("line {line:02}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(temp.path().join("tracked.txt"), base).unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        let updated = (1..=24)
            .map(|line| match line {
                2 => "changed 02".to_string(),
                20 => "changed 20".to_string(),
                _ => format!("line {line:02}"),
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(temp.path().join("tracked.txt"), updated).unwrap();
        let diff = run_git(cwd, &["diff", "--no-color", "--", "tracked.txt"]).unwrap();
        let patch = first_hunk_patch(&diff);

        git_apply_diff_hunk(cwd.to_string(), patch, false)
            .await
            .unwrap();

        let staged = run_git(cwd, &["diff", "--cached", "--no-color", "--", "tracked.txt"])
            .unwrap();
        assert!(staged.contains("+changed 02"), "{staged}");
        assert!(!staged.contains("+changed 20"), "{staged}");

        let unstaged = run_git(cwd, &["diff", "--no-color", "--", "tracked.txt"]).unwrap();
        assert!(!unstaged.contains("+changed 02"), "{unstaged}");
        assert!(unstaged.contains("+changed 20"), "{unstaged}");
    }

    #[tokio::test]
    async fn git_apply_diff_hunk_unstages_only_the_selected_staged_hunk() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        let base = (1..=24)
            .map(|line| format!("line {line:02}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(temp.path().join("tracked.txt"), base).unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        let updated = (1..=24)
            .map(|line| match line {
                2 => "changed 02".to_string(),
                20 => "changed 20".to_string(),
                _ => format!("line {line:02}"),
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(temp.path().join("tracked.txt"), updated).unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        let diff = run_git(cwd, &["diff", "--cached", "--no-color", "--", "tracked.txt"])
            .unwrap();
        let patch = first_hunk_patch(&diff);

        git_apply_diff_hunk(cwd.to_string(), patch, true)
            .await
            .unwrap();

        let staged = run_git(cwd, &["diff", "--cached", "--no-color", "--", "tracked.txt"])
            .unwrap();
        assert!(!staged.contains("+changed 02"), "{staged}");
        assert!(staged.contains("+changed 20"), "{staged}");

        let unstaged = run_git(cwd, &["diff", "--no-color", "--", "tracked.txt"]).unwrap();
        assert!(unstaged.contains("+changed 02"), "{unstaged}");
        assert!(!unstaged.contains("+changed 20"), "{unstaged}");
    }

    #[tokio::test]
    async fn git_discard_changes_removes_untracked_file() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "tracked\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();
        std::fs::write(temp.path().join("scratch.txt"), "temporary\n").unwrap();

        git_discard_changes(cwd.to_string(), vec!["scratch.txt".to_string()])
            .await
            .unwrap();

        assert!(!temp.path().join("scratch.txt").exists());
        assert!(temp.path().join("tracked.txt").exists());
        let status = run_git(cwd, &["status", "--porcelain=v1", "--untracked-files=all"]).unwrap();
        assert!(status.trim().is_empty(), "{status}");
    }

    #[tokio::test]
    async fn git_show_file_revision_returns_committed_content() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src").join("app.rs"), "committed\n").unwrap();
        run_git(cwd, &["add", "src/app.rs"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();
        std::fs::write(temp.path().join("src").join("app.rs"), "working tree\n").unwrap();

        let content = git_show_file_revision(
            cwd.to_string(),
            "src/app.rs".to_string(),
            "HEAD".to_string(),
        )
        .await
        .unwrap();

        assert_eq!(content, "committed\n");
    }

    #[tokio::test]
    async fn git_undo_last_commit_restores_message_and_keeps_changes() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "first\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "second\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "undo me"]).unwrap();

        let message = git_undo_last_commit(cwd.to_string()).await.unwrap();

        assert_eq!(message, "undo me");
        let head = run_git(cwd, &["log", "-1", "--pretty=%s"]).unwrap();
        assert_eq!(head.trim(), "initial");
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status.contains(" M tracked.txt"), "{status}");
    }

    #[tokio::test]
    async fn git_commit_amend_updates_last_commit_without_new_commit() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "first\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "amended\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();

        git_commit_amend(cwd.to_string(), "amended subject".to_string())
            .await
            .unwrap();

        let count = run_git(cwd, &["rev-list", "--count", "HEAD"]).unwrap();
        assert_eq!(count.trim(), "1");
        let subject = run_git(cwd, &["log", "-1", "--pretty=%s"]).unwrap();
        assert_eq!(subject.trim(), "amended subject");
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status.trim().is_empty(), "{status}");
    }

    #[tokio::test]
    async fn git_commit_all_amend_stages_all_changes_before_amending() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "first\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "amended\n").unwrap();
        std::fs::write(temp.path().join("new.txt"), "new file\n").unwrap();

        git_commit_all_amend(cwd.to_string(), "amend everything".to_string())
            .await
            .unwrap();

        let count = run_git(cwd, &["rev-list", "--count", "HEAD"]).unwrap();
        assert_eq!(count.trim(), "1");
        let subject = run_git(cwd, &["log", "-1", "--pretty=%s"]).unwrap();
        assert_eq!(subject.trim(), "amend everything");
        let tracked = run_git(cwd, &["show", "HEAD:tracked.txt"]).unwrap();
        assert_eq!(tracked, "amended\n");
        let added = run_git(cwd, &["show", "HEAD:new.txt"]).unwrap();
        assert_eq!(added, "new file\n");
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status.trim().is_empty(), "{status}");
    }

    #[tokio::test]
    async fn git_commit_staged_amend_preserves_unstaged_changes() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "first\n").unwrap();
        std::fs::write(temp.path().join("other.txt"), "old\n").unwrap();
        run_git(cwd, &["add", "tracked.txt", "other.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "staged amend\n").unwrap();
        std::fs::write(temp.path().join("other.txt"), "unstaged work\n").unwrap();
        std::fs::write(temp.path().join("new.txt"), "new file\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();

        git_commit_staged_amend(cwd.to_string(), "amend staged only".to_string())
            .await
            .unwrap();

        let count = run_git(cwd, &["rev-list", "--count", "HEAD"]).unwrap();
        assert_eq!(count.trim(), "1");
        let subject = run_git(cwd, &["log", "-1", "--pretty=%s"]).unwrap();
        assert_eq!(subject.trim(), "amend staged only");
        let tracked = run_git(cwd, &["show", "HEAD:tracked.txt"]).unwrap();
        assert_eq!(tracked, "staged amend\n");
        let other = run_git(cwd, &["show", "HEAD:other.txt"]).unwrap();
        assert_eq!(other, "old\n");
        let missing_untracked = run_git(cwd, &["show", "HEAD:new.txt"]);
        assert!(missing_untracked.is_err(), "{missing_untracked:?}");
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status.contains(" M other.txt"), "{status}");
        assert!(status.contains("?? new.txt"), "{status}");
    }

    #[tokio::test]
    async fn git_commit_amend_no_verify_bypasses_failing_pre_commit_hook() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "first\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        let hook_path = temp.path().join(".git").join("hooks").join("pre-commit");
        std::fs::write(&hook_path, "#!/bin/sh\necho blocked >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(temp.path().join("tracked.txt"), "amended no verify\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();

        let regular_amend = git_commit_amend(cwd.to_string(), "blocked amend".to_string()).await;
        assert!(regular_amend.is_err(), "{regular_amend:?}");

        git_commit_amend_no_verify(cwd.to_string(), "amend bypass hook".to_string())
            .await
            .unwrap();

        let count = run_git(cwd, &["rev-list", "--count", "HEAD"]).unwrap();
        assert_eq!(count.trim(), "1");
        let subject = run_git(cwd, &["log", "-1", "--pretty=%s"]).unwrap();
        assert_eq!(subject.trim(), "amend bypass hook");
        let tracked = run_git(cwd, &["show", "HEAD:tracked.txt"]).unwrap();
        assert_eq!(tracked, "amended no verify\n");
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status.trim().is_empty(), "{status}");
    }

    #[tokio::test]
    async fn git_commit_staged_amend_no_verify_preserves_unstaged_changes_and_bypasses_hook() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "first\n").unwrap();
        std::fs::write(temp.path().join("other.txt"), "old\n").unwrap();
        run_git(cwd, &["add", "tracked.txt", "other.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        let hook_path = temp.path().join(".git").join("hooks").join("pre-commit");
        std::fs::write(&hook_path, "#!/bin/sh\necho blocked >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(temp.path().join("tracked.txt"), "staged amend bypass\n").unwrap();
        std::fs::write(temp.path().join("other.txt"), "unstaged work\n").unwrap();
        std::fs::write(temp.path().join("new.txt"), "new file\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();

        git_commit_staged_amend_no_verify(cwd.to_string(), "amend staged bypass hook".to_string())
            .await
            .unwrap();

        let count = run_git(cwd, &["rev-list", "--count", "HEAD"]).unwrap();
        assert_eq!(count.trim(), "1");
        let subject = run_git(cwd, &["log", "-1", "--pretty=%s"]).unwrap();
        assert_eq!(subject.trim(), "amend staged bypass hook");
        let tracked = run_git(cwd, &["show", "HEAD:tracked.txt"]).unwrap();
        assert_eq!(tracked, "staged amend bypass\n");
        let other = run_git(cwd, &["show", "HEAD:other.txt"]).unwrap();
        assert_eq!(other, "old\n");
        let missing_untracked = run_git(cwd, &["show", "HEAD:new.txt"]);
        assert!(missing_untracked.is_err(), "{missing_untracked:?}");
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status.contains(" M other.txt"), "{status}");
        assert!(status.contains("?? new.txt"), "{status}");
    }

    #[tokio::test]
    async fn git_commit_all_amend_no_verify_stages_all_changes_and_bypasses_hook() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "first\n").unwrap();
        std::fs::write(temp.path().join("other.txt"), "old\n").unwrap();
        run_git(cwd, &["add", "tracked.txt", "other.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        let hook_path = temp.path().join(".git").join("hooks").join("pre-commit");
        std::fs::write(&hook_path, "#!/bin/sh\necho blocked >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(temp.path().join("tracked.txt"), "all amend bypass\n").unwrap();
        std::fs::write(temp.path().join("other.txt"), "all unstaged work\n").unwrap();
        std::fs::write(temp.path().join("new.txt"), "new file\n").unwrap();

        let regular_all_amend =
            git_commit_all_amend(cwd.to_string(), "blocked all amend".to_string()).await;
        assert!(regular_all_amend.is_err(), "{regular_all_amend:?}");

        git_commit_all_amend_no_verify(cwd.to_string(), "amend all bypass hook".to_string())
            .await
            .unwrap();

        let count = run_git(cwd, &["rev-list", "--count", "HEAD"]).unwrap();
        assert_eq!(count.trim(), "1");
        let subject = run_git(cwd, &["log", "-1", "--pretty=%s"]).unwrap();
        assert_eq!(subject.trim(), "amend all bypass hook");
        let tracked = run_git(cwd, &["show", "HEAD:tracked.txt"]).unwrap();
        assert_eq!(tracked, "all amend bypass\n");
        let other = run_git(cwd, &["show", "HEAD:other.txt"]).unwrap();
        assert_eq!(other, "all unstaged work\n");
        let untracked = run_git(cwd, &["show", "HEAD:new.txt"]).unwrap();
        assert_eq!(untracked, "new file\n");
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status.trim().is_empty(), "{status}");
    }

    #[tokio::test]
    async fn git_commit_no_verify_bypasses_failing_pre_commit_hook() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        let hook_path = temp.path().join(".git").join("hooks").join("pre-commit");
        std::fs::write(&hook_path, "#!/bin/sh\necho blocked >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(temp.path().join("tracked.txt"), "hook bypass\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();

        let regular_commit = git_commit(cwd.to_string(), "blocked".to_string()).await;
        assert!(regular_commit.is_err(), "{regular_commit:?}");

        git_commit_no_verify(cwd.to_string(), "bypass hook".to_string())
            .await
            .unwrap();

        let subject = run_git(cwd, &["log", "-1", "--pretty=%s"]).unwrap();
        assert_eq!(subject.trim(), "bypass hook");
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status.trim().is_empty(), "{status}");
    }

    #[tokio::test]
    async fn git_commit_signed_adds_signed_off_by_trailer() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "signed work\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();

        git_commit_signed(cwd.to_string(), "signed off work".to_string())
            .await
            .unwrap();

        let message = run_git(cwd, &["log", "-1", "--pretty=%B"]).unwrap();
        assert!(message.contains("signed off work"), "{message}");
        assert!(
            message.contains("Signed-off-by: Wardian Test <test@example.com>"),
            "{message}"
        );
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status.trim().is_empty(), "{status}");
    }

    #[tokio::test]
    async fn git_commit_staged_signed_preserves_unstaged_changes_and_adds_trailer() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "first\n").unwrap();
        std::fs::write(temp.path().join("other.txt"), "old\n").unwrap();
        run_git(cwd, &["add", "tracked.txt", "other.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        std::fs::write(temp.path().join("tracked.txt"), "signed staged\n").unwrap();
        std::fs::write(temp.path().join("other.txt"), "unstaged work\n").unwrap();
        std::fs::write(temp.path().join("new.txt"), "new file\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();

        git_commit_staged_signed(cwd.to_string(), "signed off staged work".to_string())
            .await
            .unwrap();

        let message = run_git(cwd, &["log", "-1", "--pretty=%B"]).unwrap();
        assert!(message.contains("signed off staged work"), "{message}");
        assert!(
            message.contains("Signed-off-by: Wardian Test <test@example.com>"),
            "{message}"
        );
        let tracked = run_git(cwd, &["show", "HEAD:tracked.txt"]).unwrap();
        assert_eq!(tracked, "signed staged\n");
        let other = run_git(cwd, &["show", "HEAD:other.txt"]).unwrap();
        assert_eq!(other, "old\n");
        let missing_untracked = run_git(cwd, &["show", "HEAD:new.txt"]);
        assert!(missing_untracked.is_err(), "{missing_untracked:?}");
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status.contains(" M other.txt"), "{status}");
        assert!(status.contains("?? new.txt"), "{status}");
    }

    #[tokio::test]
    async fn git_commit_all_signed_stages_all_changes_and_adds_trailer() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "first\n").unwrap();
        std::fs::write(temp.path().join("other.txt"), "old\n").unwrap();
        run_git(cwd, &["add", "tracked.txt", "other.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        std::fs::write(temp.path().join("tracked.txt"), "signed staged\n").unwrap();
        std::fs::write(temp.path().join("other.txt"), "signed unstaged\n").unwrap();
        std::fs::write(temp.path().join("new.txt"), "signed new\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();

        git_commit_all_signed(cwd.to_string(), "signed off all work".to_string())
            .await
            .unwrap();

        let message = run_git(cwd, &["log", "-1", "--pretty=%B"]).unwrap();
        assert!(message.contains("signed off all work"), "{message}");
        assert!(
            message.contains("Signed-off-by: Wardian Test <test@example.com>"),
            "{message}"
        );
        let tracked = run_git(cwd, &["show", "HEAD:tracked.txt"]).unwrap();
        assert_eq!(tracked, "signed staged\n");
        let other = run_git(cwd, &["show", "HEAD:other.txt"]).unwrap();
        assert_eq!(other, "signed unstaged\n");
        let new_file = run_git(cwd, &["show", "HEAD:new.txt"]).unwrap();
        assert_eq!(new_file, "signed new\n");
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status.trim().is_empty(), "{status}");
    }

    #[tokio::test]
    async fn git_commit_signed_no_verify_bypasses_hook_and_adds_trailer() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        let hook_path = temp.path().join(".git").join("hooks").join("pre-commit");
        std::fs::write(&hook_path, "#!/bin/sh\necho blocked >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(temp.path().join("tracked.txt"), "signed bypass\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();

        let regular_commit = git_commit_signed(cwd.to_string(), "blocked signed".to_string()).await;
        assert!(regular_commit.is_err(), "{regular_commit:?}");

        git_commit_signed_no_verify(cwd.to_string(), "signed off bypass hook".to_string())
            .await
            .unwrap();

        let message = run_git(cwd, &["log", "-1", "--pretty=%B"]).unwrap();
        assert!(message.contains("signed off bypass hook"), "{message}");
        assert!(
            message.contains("Signed-off-by: Wardian Test <test@example.com>"),
            "{message}"
        );
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status.trim().is_empty(), "{status}");
    }

    #[tokio::test]
    async fn git_commit_staged_signed_no_verify_preserves_unstaged_changes_bypasses_hook_and_adds_trailer(
    ) {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "first\n").unwrap();
        std::fs::write(temp.path().join("other.txt"), "old\n").unwrap();
        run_git(cwd, &["add", "tracked.txt", "other.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        let hook_path = temp.path().join(".git").join("hooks").join("pre-commit");
        std::fs::write(&hook_path, "#!/bin/sh\necho blocked >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(temp.path().join("tracked.txt"), "signed staged bypass\n").unwrap();
        std::fs::write(temp.path().join("other.txt"), "unstaged work\n").unwrap();
        std::fs::write(temp.path().join("new.txt"), "new file\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();

        let regular_commit =
            git_commit_staged_signed(cwd.to_string(), "blocked staged signed".to_string()).await;
        assert!(regular_commit.is_err(), "{regular_commit:?}");

        git_commit_staged_signed_no_verify(
            cwd.to_string(),
            "signed off staged bypass hook".to_string(),
        )
        .await
        .unwrap();

        let message = run_git(cwd, &["log", "-1", "--pretty=%B"]).unwrap();
        assert!(message.contains("signed off staged bypass hook"), "{message}");
        assert!(
            message.contains("Signed-off-by: Wardian Test <test@example.com>"),
            "{message}"
        );
        let tracked = run_git(cwd, &["show", "HEAD:tracked.txt"]).unwrap();
        assert_eq!(tracked, "signed staged bypass\n");
        let other = run_git(cwd, &["show", "HEAD:other.txt"]).unwrap();
        assert_eq!(other, "old\n");
        let missing_untracked = run_git(cwd, &["show", "HEAD:new.txt"]);
        assert!(missing_untracked.is_err(), "{missing_untracked:?}");
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status.contains(" M other.txt"), "{status}");
        assert!(status.contains("?? new.txt"), "{status}");
    }

    #[tokio::test]
    async fn git_commit_all_signed_no_verify_stages_all_changes_bypasses_hook_and_adds_trailer() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "first\n").unwrap();
        std::fs::write(temp.path().join("other.txt"), "old\n").unwrap();
        run_git(cwd, &["add", "tracked.txt", "other.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        let hook_path = temp.path().join(".git").join("hooks").join("pre-commit");
        std::fs::write(&hook_path, "#!/bin/sh\necho blocked >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(temp.path().join("tracked.txt"), "signed staged bypass\n").unwrap();
        std::fs::write(temp.path().join("other.txt"), "signed unstaged bypass\n").unwrap();
        std::fs::write(temp.path().join("new.txt"), "signed new bypass\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();

        let regular_commit =
            git_commit_all_signed(cwd.to_string(), "blocked all signed".to_string()).await;
        assert!(regular_commit.is_err(), "{regular_commit:?}");

        git_commit_all_signed_no_verify(cwd.to_string(), "signed off all bypass hook".to_string())
            .await
            .unwrap();

        let message = run_git(cwd, &["log", "-1", "--pretty=%B"]).unwrap();
        assert!(message.contains("signed off all bypass hook"), "{message}");
        assert!(
            message.contains("Signed-off-by: Wardian Test <test@example.com>"),
            "{message}"
        );
        let tracked = run_git(cwd, &["show", "HEAD:tracked.txt"]).unwrap();
        assert_eq!(tracked, "signed staged bypass\n");
        let other = run_git(cwd, &["show", "HEAD:other.txt"]).unwrap();
        assert_eq!(other, "signed unstaged bypass\n");
        let new_file = run_git(cwd, &["show", "HEAD:new.txt"]).unwrap();
        assert_eq!(new_file, "signed new bypass\n");
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status.trim().is_empty(), "{status}");
    }

    #[tokio::test]
    async fn git_rebase_abort_clears_in_progress_rebase_state() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("conflict.txt"), "base\n").unwrap();
        run_git(cwd, &["add", "conflict.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();
        run_git(cwd, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(temp.path().join("conflict.txt"), "feature\n").unwrap();
        run_git(cwd, &["commit", "-am", "feature change"]).unwrap();
        run_git(cwd, &["checkout", "-b", "target", "HEAD~1"]).unwrap();
        std::fs::write(temp.path().join("conflict.txt"), "main\n").unwrap();
        run_git(cwd, &["commit", "-am", "main change"]).unwrap();
        run_git(cwd, &["checkout", "feature"]).unwrap();

        run_git_allowing_status(cwd, &["rebase", "target"], &[1]).unwrap();

        let rebasing = git_status(cwd.to_string()).await.unwrap();
        assert!(rebasing.rebase_in_progress, "{rebasing:?}");

        git_rebase_abort(cwd.to_string()).await.unwrap();

        let aborted = git_status(cwd.to_string()).await.unwrap();
        assert!(!aborted.rebase_in_progress, "{aborted:?}");
        let branch = run_git(cwd, &["branch", "--show-current"]).unwrap();
        assert_eq!(branch.trim(), "feature");
    }

    #[tokio::test]
    async fn git_diff_file_against_workspace_compares_index_to_worktree() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "base\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        std::fs::write(temp.path().join("tracked.txt"), "staged\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "workspace\n").unwrap();

        let diff = git_diff_file_against_workspace(cwd.to_string(), "tracked.txt".to_string())
            .await
            .unwrap();

        assert!(diff.contains("-staged"), "{diff}");
        assert!(diff.contains("+workspace"), "{diff}");
    }

    #[tokio::test]
    async fn git_commit_staged_no_verify_preserves_unstaged_changes_and_bypasses_hook() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "first\n").unwrap();
        std::fs::write(temp.path().join("other.txt"), "old\n").unwrap();
        run_git(cwd, &["add", "tracked.txt", "other.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        let hook_path = temp.path().join(".git").join("hooks").join("pre-commit");
        std::fs::write(&hook_path, "#!/bin/sh\necho blocked >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(temp.path().join("tracked.txt"), "staged bypass\n").unwrap();
        std::fs::write(temp.path().join("other.txt"), "unstaged work\n").unwrap();
        std::fs::write(temp.path().join("new.txt"), "new file\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();

        git_commit_staged_no_verify(cwd.to_string(), "bypass hook for staged".to_string())
            .await
            .unwrap();

        let subject = run_git(cwd, &["log", "-1", "--pretty=%s"]).unwrap();
        assert_eq!(subject.trim(), "bypass hook for staged");
        let tracked = run_git(cwd, &["show", "HEAD:tracked.txt"]).unwrap();
        assert_eq!(tracked, "staged bypass\n");
        let other = run_git(cwd, &["show", "HEAD:other.txt"]).unwrap();
        assert_eq!(other, "old\n");
        let missing_untracked = run_git(cwd, &["show", "HEAD:new.txt"]);
        assert!(missing_untracked.is_err(), "{missing_untracked:?}");
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status.contains(" M other.txt"), "{status}");
        assert!(status.contains("?? new.txt"), "{status}");
    }

    #[tokio::test]
    async fn git_commit_all_no_verify_stages_all_changes_and_bypasses_hook() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        let hook_path = temp.path().join(".git").join("hooks").join("pre-commit");
        std::fs::write(&hook_path, "#!/bin/sh\necho blocked >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        std::fs::write(temp.path().join("tracked.txt"), "first\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "--no-verify", "-m", "initial"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "updated\n").unwrap();
        std::fs::write(temp.path().join("new.txt"), "new file\n").unwrap();

        git_commit_all_no_verify(cwd.to_string(), "bypass hook for all".to_string())
            .await
            .unwrap();

        let subject = run_git(cwd, &["log", "-1", "--pretty=%s"]).unwrap();
        assert_eq!(subject.trim(), "bypass hook for all");
        let tracked = run_git(cwd, &["show", "HEAD:tracked.txt"]).unwrap();
        assert_eq!(tracked, "updated\n");
        let added = run_git(cwd, &["show", "HEAD:new.txt"]).unwrap();
        assert_eq!(added, "new file\n");
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status.trim().is_empty(), "{status}");
    }

    #[tokio::test]
    async fn git_commit_empty_creates_commit_without_file_changes() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "initial\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        git_commit_empty(cwd.to_string(), "empty marker".to_string())
            .await
            .unwrap();

        let count = run_git(cwd, &["rev-list", "--count", "HEAD"]).unwrap();
        assert_eq!(count.trim(), "2");
        let subject = run_git(cwd, &["log", "-1", "--pretty=%s"]).unwrap();
        assert_eq!(subject.trim(), "empty marker");
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status.trim().is_empty(), "{status}");
    }

    #[tokio::test]
    async fn git_commit_empty_no_verify_bypasses_failing_pre_commit_hook() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "initial\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();

        let hook_path = temp.path().join(".git").join("hooks").join("pre-commit");
        std::fs::write(&hook_path, "#!/bin/sh\necho blocked >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let regular_empty = git_commit_empty(cwd.to_string(), "blocked empty".to_string()).await;
        assert!(regular_empty.is_err(), "{regular_empty:?}");

        git_commit_empty_no_verify(cwd.to_string(), "empty bypass hook".to_string())
            .await
            .unwrap();

        let count = run_git(cwd, &["rev-list", "--count", "HEAD"]).unwrap();
        assert_eq!(count.trim(), "2");
        let subject = run_git(cwd, &["log", "-1", "--pretty=%s"]).unwrap();
        assert_eq!(subject.trim(), "empty bypass hook");
        let status = run_git(cwd, &["status", "--porcelain=v1"]).unwrap();
        assert!(status.trim().is_empty(), "{status}");
    }

    #[tokio::test]
    async fn git_ignore_adds_relative_folder_pattern_to_gitignore() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        run_git(cwd, &["init"]).unwrap();
        run_git(cwd, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(cwd, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(temp.path().join("tracked.txt"), "initial\n").unwrap();
        run_git(cwd, &["add", "tracked.txt"]).unwrap();
        run_git(cwd, &["commit", "-m", "initial"]).unwrap();
        std::fs::create_dir_all(temp.path().join("logs").join("debug")).unwrap();
        std::fs::write(
            temp.path().join("logs").join("debug").join("output.log"),
            "debug\n",
        )
        .unwrap();

        git_ignore(
            cwd.to_string(),
            vec!["logs/".to_string(), "logs/".to_string()],
        )
        .await
        .unwrap();

        let gitignore = std::fs::read_to_string(temp.path().join(".gitignore")).unwrap();
        assert_eq!(gitignore, "logs/\n");
        let status = run_git(cwd, &["status", "--porcelain=v1", "--untracked-files=all"]).unwrap();
        assert!(!status.contains("logs/debug/output.log"), "{status}");
        assert!(status.contains(".gitignore"), "{status}");
    }

    #[tokio::test]
    async fn git_init_initializes_workspace_as_repository() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();

        assert!(git_status(cwd.to_string()).await.is_err());

        git_init(cwd.to_string()).await.unwrap();

        let status = git_status(cwd.to_string()).await.unwrap();
        assert!(status.files.is_empty(), "{status:?}");
        let inside = run_git(cwd, &["rev-parse", "--is-inside-work-tree"]).unwrap();
        assert_eq!(inside.trim(), "true");
    }

    #[tokio::test]
    async fn git_clone_repository_clones_into_empty_workspace() {
        let source = tempfile::tempdir().unwrap();
        let source_path = source.path().to_str().unwrap();
        run_git(source_path, &["init", "--bare"]).unwrap();
        run_git(source_path, &["symbolic-ref", "HEAD", "refs/heads/main"]).unwrap();

        let seed = tempfile::tempdir().unwrap();
        let seed_path = seed.path().to_str().unwrap();
        run_git(seed_path, &["init"]).unwrap();
        run_git(seed_path, &["config", "user.email", "test@example.com"]).unwrap();
        run_git(seed_path, &["config", "user.name", "Wardian Test"]).unwrap();
        std::fs::write(seed.path().join("README.md"), "clone me\n").unwrap();
        run_git(seed_path, &["add", "README.md"]).unwrap();
        run_git(seed_path, &["commit", "-m", "initial"]).unwrap();
        run_git(seed_path, &["push", source_path, "HEAD:main"]).unwrap();

        let target = tempfile::tempdir().unwrap();
        let target_path = target.path().to_str().unwrap();

        git_clone_repository(target_path.to_string(), source_path.to_string())
            .await
            .unwrap();

        assert!(target.path().join(".git").exists());
        let content = std::fs::read_to_string(target.path().join("README.md")).unwrap();
        assert_eq!(content.replace("\r\n", "\n"), "clone me\n");
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
    fn setup_worktree_build_caches_preserves_existing_cargo_config() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let worktree = temp.path().join("worktree");
        let cargo_dir = worktree.join(".cargo");

        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&cargo_dir).unwrap();
        std::fs::write(
            workspace.join("Cargo.toml"),
            "[package]\nname = \"sample\"\n",
        )
        .unwrap();
        std::fs::write(
            cargo_dir.join("config.toml"),
            "[build]\ntarget-dir = \"target\"\n",
        )
        .unwrap();

        setup_worktree_build_caches(&worktree, &workspace).unwrap();

        let cargo_config =
            std::fs::read_to_string(worktree.join(".cargo").join("config.toml")).unwrap();
        assert_eq!(cargo_config, "[build]\ntarget-dir = \"target\"\n");
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
