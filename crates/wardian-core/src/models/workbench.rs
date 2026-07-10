//! Typed, version-one workbench persistence DTOs and semantic validation.

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
};

/// Schema version understood by this Wardian build.
pub const WORKBENCH_SCHEMA_VERSION: u64 = 1;
/// Largest integer represented exactly by JavaScript.
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
pub const MAX_WORKBENCH_DOCUMENT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_WORKBENCH_SURFACE_STATE_BYTES: usize = 64 * 1024;
pub const MAX_WORKBENCH_TREE_DEPTH: usize = 64;
pub const MAX_RECENTLY_CLOSED_SURFACES: usize = 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Canonical device-local workbench document persisted by the Rust backend.
pub struct WorkbenchDocumentV1 {
    pub schema_version: u64,
    pub revision: u64,
    pub saved_at: String,
    pub root: WorkbenchNodeV1,
    pub groups: BTreeMap<String, WorkbenchGroupV1>,
    pub surfaces: BTreeMap<String, WorkbenchSurfaceV1>,
    pub active_group_id: String,
    pub recently_closed: Vec<ClosedSurfaceV1>,
    pub shell: WorkbenchShellV1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkbenchNodeV1 {
    Group {
        group_id: String,
    },
    Split {
        node_id: String,
        direction: WorkbenchSplitDirection,
        ratio: f64,
        first: Box<WorkbenchNodeV1>,
        second: Box<WorkbenchNodeV1>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkbenchSplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkbenchGroupV1 {
    pub group_id: String,
    pub surface_ids: Vec<String>,
    pub active_surface_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkbenchSurfaceV1 {
    pub surface_id: String,
    pub surface_type: String,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_non_null_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub resource_key: Option<String>,
    pub state_schema_version: u64,
    pub state: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClosedSurfaceV1 {
    pub surface: WorkbenchSurfaceV1,
    pub previous_group_id: String,
    pub previous_index: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkbenchShellV1 {
    pub left_sidebar_collapsed: bool,
    pub left_sidebar_width: f64,
    pub right_sidebar_collapsed: bool,
    pub right_sidebar_width: f64,
    pub bottom_terminal_open: bool,
    pub bottom_terminal_height: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkbenchValidationError {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkbenchValidationErrors {
    pub errors: Vec<WorkbenchValidationError>,
}

impl fmt::Display for WorkbenchValidationErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "workbench validation failed with {} error(s)",
            self.errors.len()
        )?;
        if let Some(error) = self.errors.first() {
            write!(formatter, ": {} {}", error.path, error.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for WorkbenchValidationErrors {}

impl WorkbenchDocumentV1 {
    /// Builds the deterministic revision-zero document used for first launch.
    pub fn default_document() -> Self {
        let group_id = "group-1".to_string();
        Self {
            schema_version: WORKBENCH_SCHEMA_VERSION,
            revision: 0,
            saved_at: "1970-01-01T00:00:00.000Z".to_string(),
            root: WorkbenchNodeV1::Group {
                group_id: group_id.clone(),
            },
            groups: BTreeMap::from([(
                group_id.clone(),
                WorkbenchGroupV1 {
                    group_id: group_id.clone(),
                    surface_ids: Vec::new(),
                    active_surface_id: None,
                },
            )]),
            surfaces: BTreeMap::new(),
            active_group_id: group_id,
            recently_closed: Vec::new(),
            shell: WorkbenchShellV1 {
                left_sidebar_collapsed: false,
                left_sidebar_width: 240.0,
                right_sidebar_collapsed: false,
                right_sidebar_width: 240.0,
                bottom_terminal_open: false,
                bottom_terminal_height: 360.0,
            },
        }
    }

    /// Validates the full V1 graph without normalizing or dropping opaque state.
    pub fn validate(&self) -> Result<(), WorkbenchValidationErrors> {
        let mut errors = Vec::new();

        if self.schema_version != WORKBENCH_SCHEMA_VERSION {
            add_error(&mut errors, "$.schema_version", "must equal 1");
        }
        validate_safe_integer(self.revision, "$.revision", &mut errors);
        if !is_canonical_timestamp(&self.saved_at) {
            add_error(
                &mut errors,
                "$.saved_at",
                "must be a canonical UTC millisecond timestamp",
            );
        }

        let mut group_references = Vec::new();
        let mut split_ids = HashSet::new();
        validate_node(
            &self.root,
            1,
            "$.root",
            &mut group_references,
            &mut split_ids,
            &mut errors,
        );

        let mut open_surface_references = Vec::new();
        for (group_key, group) in &self.groups {
            let path = format!("$.groups.{group_key}");
            if group.group_id != *group_key {
                add_error(
                    &mut errors,
                    format!("{path}.group_id"),
                    "must match its record key",
                );
            }
            let mut local_ids = HashSet::new();
            for (index, surface_id) in group.surface_ids.iter().enumerate() {
                if surface_id.is_empty() {
                    add_error(
                        &mut errors,
                        format!("{path}.surface_ids[{index}]"),
                        "must be a non-empty string",
                    );
                } else {
                    if !local_ids.insert(surface_id.as_str()) {
                        add_error(
                            &mut errors,
                            format!("{path}.surface_ids[{index}]"),
                            "must not be duplicated in a group",
                        );
                    }
                    open_surface_references.push(surface_id.as_str());
                }
            }
            if group.surface_ids.is_empty() {
                if group.active_surface_id.is_some() {
                    add_error(
                        &mut errors,
                        format!("{path}.active_surface_id"),
                        "must be null for an empty group",
                    );
                }
            } else if group
                .active_surface_id
                .as_deref()
                .is_none_or(|active| !local_ids.contains(active))
            {
                add_error(
                    &mut errors,
                    format!("{path}.active_surface_id"),
                    "must reference a tab in the group",
                );
            }
        }

        let group_counts = reference_counts(&group_references);
        for group_id in self.groups.keys() {
            if group_counts.get(group_id.as_str()).copied() != Some(1) {
                add_error(
                    &mut errors,
                    format!("$.groups.{group_id}"),
                    "must be referenced exactly once by the tree",
                );
            }
        }
        for group_id in &group_references {
            if !self.groups.contains_key(*group_id) {
                add_error(
                    &mut errors,
                    "$.root",
                    format!("references missing group {group_id}"),
                );
            }
        }

        for (surface_key, surface) in &self.surfaces {
            let path = format!("$.surfaces.{surface_key}");
            validate_surface(surface, &path, &mut errors);
            if surface.surface_id != *surface_key {
                add_error(
                    &mut errors,
                    format!("{path}.surface_id"),
                    "must match its record key",
                );
            }
        }
        let surface_counts = reference_counts(&open_surface_references);
        for surface_id in self.surfaces.keys() {
            if surface_counts.get(surface_id.as_str()).copied() != Some(1) {
                add_error(
                    &mut errors,
                    format!("$.surfaces.{surface_id}"),
                    "must be referenced exactly once by a group",
                );
            }
        }
        for surface_id in &open_surface_references {
            if !self.surfaces.contains_key(*surface_id) {
                add_error(
                    &mut errors,
                    "$.groups",
                    format!("references missing surface {surface_id}"),
                );
            }
        }

        if self.active_group_id.is_empty()
            || !self.groups.contains_key(&self.active_group_id)
            || !group_references.contains(&self.active_group_id.as_str())
        {
            add_error(
                &mut errors,
                "$.active_group_id",
                "must reference a tree group",
            );
        }

        if self.recently_closed.len() > MAX_RECENTLY_CLOSED_SURFACES {
            add_error(
                &mut errors,
                "$.recently_closed",
                "must contain at most 20 surfaces",
            );
        }
        for (index, closed) in self.recently_closed.iter().enumerate() {
            let path = format!("$.recently_closed[{index}]");
            validate_surface(&closed.surface, &format!("{path}.surface"), &mut errors);
            if closed.previous_group_id.is_empty() {
                add_error(
                    &mut errors,
                    format!("{path}.previous_group_id"),
                    "must be a non-empty string",
                );
            }
            validate_safe_integer(
                closed.previous_index,
                &format!("{path}.previous_index"),
                &mut errors,
            );
        }

        validate_shell(&self.shell, &mut errors);

        if errors.is_empty() {
            match serde_json::to_vec(self) {
                Ok(bytes) if bytes.len() > MAX_WORKBENCH_DOCUMENT_BYTES => {
                    add_error(&mut errors, "$", "exceeds the 2 MiB UTF-8 document limit")
                }
                Err(_) => add_error(&mut errors, "$", "must be serializable JSON"),
                _ => {}
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(WorkbenchValidationErrors { errors })
        }
    }
}

fn deserialize_optional_non_null_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

fn add_error(
    errors: &mut Vec<WorkbenchValidationError>,
    path: impl Into<String>,
    message: impl Into<String>,
) {
    errors.push(WorkbenchValidationError {
        path: path.into(),
        message: message.into(),
    });
}

fn validate_safe_integer(value: u64, path: &str, errors: &mut Vec<WorkbenchValidationError>) {
    if value > MAX_SAFE_INTEGER {
        add_error(errors, path, "must be a non-negative safe integer");
    }
}

fn validate_node<'a>(
    node: &'a WorkbenchNodeV1,
    depth: usize,
    path: &str,
    group_references: &mut Vec<&'a str>,
    split_ids: &mut HashSet<&'a str>,
    errors: &mut Vec<WorkbenchValidationError>,
) {
    if depth > MAX_WORKBENCH_TREE_DEPTH {
        add_error(errors, path, "exceeds the 64-node tree depth limit");
        return;
    }
    match node {
        WorkbenchNodeV1::Group { group_id } => {
            if group_id.is_empty() {
                add_error(
                    errors,
                    format!("{path}.group_id"),
                    "must be a non-empty string",
                );
            } else {
                group_references.push(group_id);
            }
        }
        WorkbenchNodeV1::Split {
            node_id,
            ratio,
            first,
            second,
            ..
        } => {
            if node_id.is_empty() {
                add_error(
                    errors,
                    format!("{path}.node_id"),
                    "must be a non-empty string",
                );
            } else if !split_ids.insert(node_id) {
                add_error(errors, format!("{path}.node_id"), "must be unique");
            }
            if !ratio.is_finite() || !(0.1..=0.9).contains(ratio) {
                add_error(
                    errors,
                    format!("{path}.ratio"),
                    "must be a finite number in 0.1..0.9",
                );
            }
            validate_node(
                first,
                depth + 1,
                &format!("{path}.first"),
                group_references,
                split_ids,
                errors,
            );
            validate_node(
                second,
                depth + 1,
                &format!("{path}.second"),
                group_references,
                split_ids,
                errors,
            );
        }
    }
}

fn validate_surface(
    surface: &WorkbenchSurfaceV1,
    path: &str,
    errors: &mut Vec<WorkbenchValidationError>,
) {
    if surface.surface_id.is_empty() {
        add_error(
            errors,
            format!("{path}.surface_id"),
            "must be a non-empty string",
        );
    }
    if surface.surface_type.is_empty() {
        add_error(
            errors,
            format!("{path}.surface_type"),
            "must be a non-empty string",
        );
    }
    validate_safe_integer(
        surface.state_schema_version,
        &format!("{path}.state_schema_version"),
        errors,
    );
    validate_json_value(&surface.state, &format!("{path}.state"), errors);
    match serde_json::to_vec(&surface.state) {
        Ok(bytes) if bytes.len() > MAX_WORKBENCH_SURFACE_STATE_BYTES => add_error(
            errors,
            format!("{path}.state"),
            "exceeds the 64 KiB UTF-8 limit",
        ),
        Err(_) => add_error(errors, format!("{path}.state"), "must be serializable JSON"),
        _ => {}
    }
}

fn validate_json_value(
    value: &serde_json::Value,
    path: &str,
    errors: &mut Vec<WorkbenchValidationError>,
) {
    let mut stack = vec![(value, path.to_string())];
    while let Some((current, current_path)) = stack.pop() {
        match current {
            serde_json::Value::Number(number) => {
                let finite = number.as_f64().is_some_and(f64::is_finite);
                let negative_zero = number
                    .as_f64()
                    .is_some_and(|value| value == 0.0 && value.is_sign_negative());
                if !finite || negative_zero {
                    add_error(
                        errors,
                        current_path,
                        "must contain only round-trippable finite JSON numbers",
                    );
                }
            }
            serde_json::Value::Array(values) => {
                for (index, child) in values.iter().enumerate().rev() {
                    stack.push((child, format!("{current_path}[{index}]")));
                }
            }
            serde_json::Value::Object(values) => {
                for (key, child) in values.iter().rev() {
                    stack.push((child, format!("{current_path}.{key}")));
                }
            }
            _ => {}
        }
    }
}

fn validate_shell(shell: &WorkbenchShellV1, errors: &mut Vec<WorkbenchValidationError>) {
    for (path, value) in [
        ("$.shell.left_sidebar_width", shell.left_sidebar_width),
        ("$.shell.right_sidebar_width", shell.right_sidebar_width),
        (
            "$.shell.bottom_terminal_height",
            shell.bottom_terminal_height,
        ),
    ] {
        if !value.is_finite() || value < 0.0 || (value == 0.0 && value.is_sign_negative()) {
            add_error(errors, path, "must be a finite non-negative number");
        }
    }
}

fn reference_counts<'a>(values: &[&'a str]) -> HashMap<&'a str, usize> {
    let mut counts = HashMap::new();
    for value in values {
        *counts.entry(*value).or_insert(0) += 1;
    }
    counts
}

fn is_canonical_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 24
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'.'
        || bytes[23] != b'Z'
    {
        return false;
    }
    let digit_ranges = [0..4, 5..7, 8..10, 11..13, 14..16, 17..19, 20..23];
    if digit_ranges
        .iter()
        .any(|range| !bytes[range.clone()].iter().all(u8::is_ascii_digit))
    {
        return false;
    }
    let parse = |range: std::ops::Range<usize>| {
        value[range]
            .parse::<u32>()
            .expect("timestamp range contains only ASCII digits")
    };
    let year = parse(0..4) as i32;
    let month = parse(5..7);
    let day = parse(8..10);
    let hour = parse(11..13);
    let minute = parse(14..16);
    let second = parse(17..19);
    let millisecond = parse(20..23);
    chrono::NaiveDate::from_ymd_opt(year, month, day).is_some()
        && chrono::NaiveTime::from_hms_milli_opt(hour, minute, second, millisecond).is_some()
}
