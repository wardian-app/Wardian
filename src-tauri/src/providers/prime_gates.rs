//! Derives Prime Agent autonomous gates from a project's `AGENTS.md`.
//!
//! Wardian already records each project's verification commands in the
//! pre-commit checklist. Prime can enforce them: a failed `--autonomous-gate`
//! feeds bounded output into the next continuation so the agent repairs it,
//! and a passing gate permits completion. Reading the checklist rather than
//! asking for a second copy keeps one source of truth.

/// Upper bound on gates emitted for one agent.
///
/// Every gate is a command Prime runs at the end of a turn, so a long
/// checklist would multiply turn cost without adding signal. The limit favours
/// the earliest entries, which is where checklists put the cheap checks.
pub const MAX_GATES: usize = 6;

/// Command runners a checklist entry may start with.
///
/// A checklist is prose with inline code in it, so most backticked spans are
/// filenames, flags, or commit-message examples rather than commands. Matching
/// on a known runner is what separates `npm run lint` from `AGENTS.md` and
/// `feat(workflows):`.
const COMMAND_RUNNERS: &[&str] = &[
    "npm", "pnpm", "yarn", "bun", "npx", "cargo", "rustup", "go", "make", "just", "python",
    "python3", "pytest", "tox", "ruff", "mypy", "dotnet", "mvn", "gradle", "swift", "ctest",
    "cmake", "composer", "bundle", "rake", "mix", "deno",
];

/// Shell operators that mean an entry is a compound or an example rather than
/// a single verification command.
///
/// Each gate is emitted as its own argument, so a compound would either be
/// mangled or run a second command the checklist never separated out.
const SHELL_OPERATORS: &[&str] = &["&&", "||", ";", "|", "$(", "`", ">", "<", "\n"];

/// Extracts verification commands from an `AGENTS.md` body.
///
/// Scans only the pre-commit checklist section. Commands elsewhere in the file
/// are prose about how the project works, not a contract a turn should be
/// gated on, and running them unbidden would be a surprise.
pub fn gates_from_agents_md(contents: &str) -> Vec<String> {
    let mut gates: Vec<String> = Vec::new();
    let mut in_checklist = false;
    let mut checklist_depth = 0usize;

    for line in contents.lines() {
        if let Some((depth, heading)) = markdown_heading(line) {
            // A heading at the same or shallower level ends the section.
            if in_checklist && depth <= checklist_depth {
                in_checklist = false;
            }
            if is_verification_heading(&heading) {
                in_checklist = true;
                checklist_depth = depth;
            }
            continue;
        }

        if !in_checklist {
            continue;
        }

        for candidate in inline_code_spans(line) {
            if !is_verification_command(&candidate) {
                continue;
            }
            if gates.iter().any(|gate| gate == &candidate) {
                continue;
            }
            gates.push(candidate);
            if gates.len() == MAX_GATES {
                return gates;
            }
        }
    }

    gates
}

/// Splits a heading line into its level and text.
fn markdown_heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    let depth = trimmed.chars().take_while(|c| *c == '#').count();
    if depth == 0 || depth > 6 {
        return None;
    }
    // `#hashtag` is not a heading; a heading has whitespace after its hashes.
    if !trimmed[depth..].starts_with(char::is_whitespace) {
        return None;
    }

    Some((depth, trimmed[depth..].trim().to_string()))
}

fn is_verification_heading(heading: &str) -> bool {
    let lowered = heading.to_ascii_lowercase();
    (lowered.contains("pre-commit")
        || lowered.contains("precommit")
        || lowered.contains("checklist")
        || lowered.contains("verification"))
        && !lowered.contains("skip")
}

/// Returns the contents of each single-backtick span in a line.
fn inline_code_spans(line: &str) -> Vec<String> {
    let mut spans = Vec::new();
    let mut rest = line;

    while let Some(open) = rest.find('`') {
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('`') else {
            break;
        };
        let span = after_open[..close].trim();
        if !span.is_empty() {
            spans.push(span.to_string());
        }
        rest = &after_open[close + 1..];
    }

    spans
}

/// True when a span reads as a runnable verification command.
fn is_verification_command(candidate: &str) -> bool {
    if candidate.len() > 200 {
        return false;
    }
    if SHELL_OPERATORS
        .iter()
        .any(|operator| candidate.contains(operator))
    {
        return false;
    }

    let mut words = candidate.split_whitespace();
    let Some(runner) = words.next() else {
        return false;
    };
    // A bare runner is not a check; `cargo` alone verifies nothing.
    if words.next().is_none() {
        return false;
    }

    COMMAND_RUNNERS.contains(&runner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_projects_own_checklist() {
        // Shaped after Wardian's AGENTS.md, including the parenthetical that
        // puts a bare directory name in backticks next to real commands.
        let contents = r#"
# Project Guidelines

Use the `replace` tool for edits and see `docs/specs/`.

## Pre-Commit Checklist

1. **Validation & Build**:
   - [ ] **Frontend**: Run `npm run lint`, `npm run test`, and `npm run build`.
   - [ ] **Backend**: Run `cargo clippy`, `cargo test`, and `cargo check` (in `src-tauri`).

## Architecture

Run `npm run dev` to start the app.
"#;

        let gates = gates_from_agents_md(contents);

        assert_eq!(
            gates,
            vec![
                "npm run lint",
                "npm run test",
                "npm run build",
                "cargo clippy",
                "cargo test",
                "cargo check",
            ]
        );
    }

    #[test]
    fn prose_backticks_are_not_commands() {
        let contents = r#"
## Pre-Commit Checklist

- [ ] Use a message like `feat(workflows): add thing`.
- [ ] Check `AGENTS.md` and `src/types/index.ts`.
- [ ] Confirm the `--json` flag is set.
- [ ] Run `cargo test`.
"#;

        // Only the actual command survives; a filename or a commit-message
        // example would be nonsense to run as a gate.
        assert_eq!(gates_from_agents_md(contents), vec!["cargo test"]);
    }

    #[test]
    fn commands_outside_the_checklist_are_left_alone() {
        let contents = r#"
## Getting Started

Run `npm install` and then `cargo build`.

## Pre-Commit Checklist

- [ ] Run `npm run lint`.

## Troubleshooting

If it hangs, run `cargo clean`.
"#;

        // Gating a turn on a setup or cleanup command would be a surprise the
        // checklist never asked for.
        assert_eq!(gates_from_agents_md(contents), vec!["npm run lint"]);
    }

    #[test]
    fn compound_commands_are_skipped_rather_than_mangled() {
        let contents = r#"
## Pre-Commit Checklist

- [ ] Run `npm run lint && npm run test`.
- [ ] Run `cargo test 2> log.txt`.
- [ ] Run `npm run build`.
"#;

        // Each gate is one argument, so a compound would either be mangled or
        // quietly run a second command.
        assert_eq!(gates_from_agents_md(contents), vec!["npm run build"]);
    }

    #[test]
    fn a_bare_runner_is_not_a_check() {
        let contents =
            "## Checklist\n\n- [ ] Know your `cargo` and `npm`.\n- [ ] Run `cargo test`.\n";

        assert_eq!(gates_from_agents_md(contents), vec!["cargo test"]);
    }

    #[test]
    fn duplicates_collapse_and_the_list_stays_bounded() {
        let mut contents = String::from("## Pre-Commit Checklist\n\n- [ ] Run `cargo test`.\n");
        for index in 0..20 {
            contents.push_str(&format!("- [ ] Run `npm run check-{index}`.\n"));
        }
        contents.push_str("- [ ] Run `cargo test` again.\n");

        let gates = gates_from_agents_md(&contents);

        assert_eq!(gates.len(), MAX_GATES);
        assert_eq!(gates[0], "cargo test");
        // Every gate costs a command at the end of each turn.
        assert_eq!(gates.iter().filter(|gate| *gate == "cargo test").count(), 1);
    }

    #[test]
    fn a_file_with_no_checklist_produces_no_gates() {
        assert!(gates_from_agents_md("# Guidelines\n\nRun `npm test` sometimes.\n").is_empty());
        assert!(gates_from_agents_md("").is_empty());
    }

    #[test]
    fn nested_headings_stay_inside_the_checklist() {
        let contents = r#"
## Pre-Commit Checklist

### Frontend

- [ ] Run `npm run lint`.

### Backend

- [ ] Run `cargo clippy`.

## Architecture

- Run `npm run dev`.
"#;

        // A deeper heading subdivides the section; a same-level one ends it.
        assert_eq!(
            gates_from_agents_md(contents),
            vec!["npm run lint", "cargo clippy"]
        );
    }
}
