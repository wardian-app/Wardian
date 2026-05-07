use std::io;

use wardian_core::models::WorkflowDefinition;

pub fn list_workflows_from_disk() -> io::Result<Vec<WorkflowDefinition>> {
    let home = wardian_core::paths::wardian_home()
        .ok_or_else(|| io::Error::other("WARDIAN_HOME not set"))?;
    let workflows_dir = home.join("workflows");
    if !workflows_dir.exists() {
        return Ok(vec![]);
    }

    let mut workflows = vec![];
    for entry in std::fs::read_dir(&workflows_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        let content = std::fs::read_to_string(&path)?;
        match serde_json::from_str::<WorkflowDefinition>(&content) {
            Ok(workflow) => workflows.push(workflow),
            Err(error) => eprintln!(
                "warning: skipped malformed workflow file {}: {}",
                path.display(),
                error
            ),
        }
    }
    Ok(workflows)
}

pub fn workflow_summaries(
    workflows: &[WorkflowDefinition],
) -> Vec<wardian_core::control::WorkflowSummary> {
    workflows
        .iter()
        .map(|workflow| wardian_core::control::WorkflowSummary {
            id: workflow.id.clone(),
            name: workflow.name.clone(),
            node_count: workflow.nodes.len(),
        })
        .collect()
}
