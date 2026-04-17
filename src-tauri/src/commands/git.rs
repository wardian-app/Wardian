use crate::models::git::{GitFileEntry, GitLogEntry, GitStatusResult};
use crate::utils::process::new_headless_std_command;

/// Run a git command in the given directory and return stdout as a String.
///
/// Uses `new_headless_std_command` so no console window flashes on Windows.
/// Sets `GIT_TERMINAL_PROMPT=0` and `GIT_ASKPASS=echo` so git never blocks
/// waiting for credential input (mirrors VS Code's git extension behaviour).
fn run_git(cwd: &str, args: &[&str]) -> Result<String, String> {
    let output = new_headless_std_command("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "echo")
        .output()
        .map_err(|e| format!("Failed to execute git: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stderr.trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Parse `git status --porcelain=v1 -b` output into a GitStatusResult.
fn parse_porcelain_status(raw: &str) -> GitStatusResult {
    let mut branch = String::new();
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
        let path = line[3..].to_string();

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
        files,
        ahead,
        behind,
    }
}

#[tauri::command]
pub async fn git_status(cwd: String) -> Result<GitStatusResult, String> {
    let raw = run_git(&cwd, &["status", "--porcelain=v1", "-b", "--untracked-files=all"])?;
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
        let file_path = &line[3..];
        if !path_set.contains(file_path) {
            continue;
        }
        if line.starts_with("??") {
            untracked.push(file_path.to_string());
        } else {
            tracked.push(file_path.to_string());
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
    run_git(&cwd, &["push"])
}

#[tauri::command]
pub async fn git_create_worktree(
    cwd: String,
    path: String,
    branch: String,
) -> Result<(), String> {
    run_git(&cwd, &["worktree", "add", &path, "-b", &branch])?;
    Ok(())
}

#[tauri::command]
pub async fn git_remove_worktree(cwd: String, path: String) -> Result<(), String> {
    run_git(&cwd, &["worktree", "remove", &path, "--force"])?;
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
        assert_eq!(result.ahead, 0);
        assert_eq!(result.behind, 0);
        // MM = staged M + unstaged M
        assert_eq!(result.files.len(), 2);
        assert!(result.files[0].is_staged);
        assert!(!result.files[1].is_staged);
    }
}
