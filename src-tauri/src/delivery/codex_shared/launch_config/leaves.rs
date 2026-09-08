//! Restrict journal contents to the trusted generator's scalar/string-array leaves.
use super::{failure, CodexSharedError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use toml_edit::{value, Array, DocumentMut, Item, Table, TableLike};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(super) enum Leaf {
    String(String),
    Strings(Vec<String>),
}

impl Leaf {
    pub(super) fn read(item: &Item) -> Option<Self> {
        if let Some(text) = item.as_str() {
            return Some(Self::String(text.to_owned()));
        }
        item.as_array().and_then(|array| {
            array
                .iter()
                .map(|entry| entry.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .map(Self::Strings)
        })
    }

    fn item(&self) -> Item {
        match self {
            Self::String(text) => value(text),
            Self::Strings(strings) => {
                let mut array = Array::new();
                for text in strings {
                    array.push(text.as_str());
                }
                value(array)
            }
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Change {
    pub(super) path: Vec<String>,
    pub(super) before: Option<Leaf>,
    pub(super) applied: Leaf,
}

pub(super) type Overrides = BTreeMap<Vec<String>, Leaf>;

pub(super) fn validate(path: &[String], leaf: &Leaf) -> Result<(), CodexSharedError> {
    let allowed = match path {
        [key] => {
            matches!(
                key.as_str(),
                "model"
                    | "model_reasoning_effort"
                    | "sandbox_mode"
                    | "approval_policy"
                    | "approvals_reviewer"
                    | "web_search"
                    | "developer_instructions"
            ) && matches!(leaf, Leaf::String(_))
        }
        [table, key] => {
            table == "sandbox_workspace_write"
                && key == "writable_roots"
                && matches!(leaf, Leaf::Strings(_))
        }
        [table, project, key] => {
            table == "projects"
                && !project.is_empty()
                && key == "trust_level"
                && matches!(leaf, Leaf::String(_))
        }
        _ => false,
    };
    if !allowed {
        return Err(failure("unexpected generated Codex launch leaf or type"));
    }
    Ok(())
}

pub(super) fn parse(args: &[String]) -> Result<Overrides, CodexSharedError> {
    if args.first().map(String::as_str) != Some("app-server") || args.len() % 2 != 1 {
        return Err(failure(
            "expected generated app-server and -c key=value pairs",
        ));
    }
    let mut result = BTreeMap::new();
    for pair in args[1..].chunks_exact(2) {
        if pair[0] != "-c" {
            return Err(failure("unexpected non-config Codex launch argument"));
        }
        let document = pair[1]
            .parse::<DocumentMut>()
            .map_err(|_| failure("invalid generated Codex launch TOML"))?;
        let mut branch = document.as_table();
        let mut path = Vec::new();
        loop {
            if branch.len() != 1 {
                return Err(failure("each -c must contain one generated assignment"));
            }
            let (key, item) = branch.iter().next().expect("one assignment");
            path.push(key.to_owned());
            if let Item::Table(child) = item {
                if !child.is_dotted() {
                    return Err(failure("table headers are not generated assignments"));
                }
                branch = child;
            } else {
                let leaf = Leaf::read(item)
                    .ok_or_else(|| failure("unsupported generated Codex launch value"))?;
                validate(&path, &leaf)?;
                result.insert(path, leaf); // Last duplicate wins, before is captured only once.
                break;
            }
        }
    }
    Ok(result)
}

fn at<'a>(table: &'a dyn TableLike, path: &[String]) -> Result<Option<&'a Item>, ()> {
    let Some(item) = table.get(&path[0]) else {
        return Ok(None);
    };
    if path.len() == 1 {
        return Ok(Some(item));
    }
    at(item.as_table_like().ok_or(())?, &path[1..])
}

pub(super) fn apply(
    document: &mut DocumentMut,
    overrides: Overrides,
) -> Result<Vec<Change>, CodexSharedError> {
    let mut changes = Vec::new();
    for (path, applied) in overrides {
        let before = at(document.as_table(), &path)
            .map_err(|()| failure("Codex launch key crosses a non-table value"))?
            .map(|item| {
                Leaf::read(item)
                    .ok_or_else(|| failure("existing Codex launch leaf has an unsupported shape"))
            })
            .transpose()?;
        if let Some(leaf) = &before {
            validate(&path, leaf)?;
        }
        if before.as_ref() != Some(&applied) {
            set(document.as_table_mut(), &path, &applied)?;
            changes.push(Change {
                path,
                before,
                applied,
            });
        }
    }
    Ok(changes)
}

fn set(
    table: &mut dyn TableLike,
    path: &[String],
    replacement: &Leaf,
) -> Result<(), CodexSharedError> {
    let key = &path[0];
    if path.len() == 1 {
        let mut item = replacement.item();
        if let Some(existing) = table.get_mut(key) {
            let decoration = existing
                .as_value()
                .ok_or_else(|| failure("Codex launch leaf became a table"))?
                .decor()
                .clone();
            *item.as_value_mut().expect("generated leaf").decor_mut() = decoration;
            *existing = item;
        } else {
            table.insert(key, item);
        }
        return Ok(());
    }
    if !table.contains_key(key) {
        let mut child = Table::new();
        child.set_implicit(true);
        table.insert(key, Item::Table(child));
    }
    let child = table
        .get_mut(key)
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| failure("Codex launch key crosses a non-table value"))?;
    set(child, &path[1..], replacement)
}

fn remove(table: &mut dyn TableLike, path: &[String]) {
    if path.len() == 1 {
        table.remove(&path[0]);
    } else if let Some(child) = table.get_mut(&path[0]).and_then(Item::as_table_like_mut) {
        remove(child, &path[1..]);
    }
    // Retain empty parents: removing their comments or foreign siblings is not
    // needed to revoke a generated leaf, and the journal need not copy tables.
}

pub(super) fn restore(
    document: &mut DocumentMut,
    changes: &[Change],
) -> Result<(), CodexSharedError> {
    for change in changes {
        let owned = at(document.as_table(), &change.path)
            .ok()
            .flatten()
            .and_then(Leaf::read)
            .as_ref()
            == Some(&change.applied);
        if !owned {
            continue;
        } // Changed value, shape or deletion is the new baseline.
        if let Some(before) = &change.before {
            set(document.as_table_mut(), &change.path, before)?;
        } else {
            remove(document.as_table_mut(), &change.path);
        }
    }
    Ok(())
}
