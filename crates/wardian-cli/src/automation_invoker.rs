//! Validation and parsing shared by every automation invoker CLI surface.
//!
//! Schedules, session-close invokers, and listeners all accept the same
//! blueprint, provider, input, and binding arguments. Keeping the checks here
//! rather than in `main` means a new invoker family reuses them by importing
//! rather than by threading callbacks through the command dispatcher.

use crate::args::ScheduleDefinitionArgs;
use crate::errors::CliError;
use crate::json_input;
use std::collections::HashMap;
use wardian_core::models::ScheduleDefinition;

pub(crate) fn parse_automation_exec_input(
    input: Option<&str>,
) -> Result<serde_json::Value, CliError> {
    let Some(raw) = input else {
        return Ok(serde_json::json!({}));
    };
    json_input::parse(raw, "--input")
}

pub(crate) fn build_schedule_definition(
    args: &ScheduleDefinitionArgs,
    base: Option<&ScheduleDefinition>,
) -> Result<ScheduleDefinition, CliError> {
    let mut definition = base.cloned().unwrap_or_else(|| ScheduleDefinition {
        active: true,
        ..Default::default()
    });
    let cadence_set = schedule_definition_args_set(args);
    if !cadence_set && base.is_none() {
        return Err(CliError::generic(
            "specify one of --every / --daily / --weekly / --monthly / --specific-dates / --at",
        ));
    }
    if let Some(minutes) = args.every {
        definition.schedule_type = "interval".into();
        definition.interval_minutes = Some(minutes);
        definition.time_of_day = None;
        definition.days_of_week = None;
        definition.days_of_month = None;
        definition.specific_dates = None;
        definition.run_at = None;
    } else if let Some(time) = args.daily.clone() {
        definition.schedule_type = "daily".into();
        definition.time_of_day = Some(time);
        definition.interval_minutes = None;
        definition.days_of_week = None;
        definition.days_of_month = None;
        definition.specific_dates = None;
        definition.run_at = None;
    } else if let Some(spec) = args.weekly.as_deref() {
        let (days, time) = spec
            .split_once('@')
            .ok_or_else(|| CliError::generic("--weekly expects Days@HH:MM, e.g. Mon,Fri@09:30"))?;
        definition.schedule_type = "weekly".into();
        definition.days_of_week = Some(days.split(',').map(|day| day.trim().to_string()).collect());
        definition.time_of_day = Some(time.to_string());
        definition.interval_minutes = None;
        definition.days_of_month = None;
        definition.specific_dates = None;
        definition.run_at = None;
        if base.is_none_or(|base| base.schedule_type != "weekly") {
            definition.repeat_every = 1;
        }
    } else if let Some(spec) = args.monthly.as_deref() {
        let (days, time) = spec.split_once('@').ok_or_else(|| {
            CliError::generic("--monthly expects day numbers@HH:MM, e.g. 1,15@09:30")
        })?;
        let days = days
            .split(',')
            .map(|day| {
                day.trim()
                    .parse::<u32>()
                    .map_err(|_| CliError::generic(format!("invalid monthly day `{}`", day.trim())))
            })
            .collect::<Result<Vec<_>, _>>()?;
        definition.schedule_type = "monthly".into();
        definition.days_of_month = Some(days);
        definition.time_of_day = Some(time.to_string());
        definition.interval_minutes = None;
        definition.days_of_week = None;
        definition.specific_dates = None;
        definition.run_at = None;
    } else if let Some(spec) = args.specific_dates.as_deref() {
        let (dates, time) = spec.split_once('@').ok_or_else(|| {
            CliError::generic(
                "--specific-dates expects YYYY-MM-DD dates@HH:MM, e.g. 2026-09-01,2026-09-15@09:30",
            )
        })?;
        let dates = dates
            .split(',')
            .map(|date| date.trim().to_string())
            .collect::<Vec<_>>();
        definition.schedule_type = "specific_dates".into();
        definition.specific_dates = Some(dates);
        definition.time_of_day = Some(time.to_string());
        definition.interval_minutes = None;
        definition.days_of_week = None;
        definition.days_of_month = None;
        definition.run_at = None;
    } else if let Some(when) = args.at.clone() {
        definition.schedule_type = "one_time".into();
        definition.run_at = Some(when);
        definition.interval_minutes = None;
        definition.time_of_day = None;
        definition.days_of_week = None;
        definition.days_of_month = None;
        definition.specific_dates = None;
    }
    if let Some(repeat_every) = args.repeat_every {
        if repeat_every == 0 {
            return Err(CliError::generic(
                "--repeat-every must be greater than zero",
            ));
        }
        if definition.schedule_type != "weekly" {
            return Err(CliError::generic(
                "--repeat-every is only supported for weekly schedules",
            ));
        }
        definition.repeat_every = repeat_every;
    }
    apply_end_condition(args, &mut definition)?;
    if definition.end_condition.trim().is_empty() {
        definition.end_condition = "never".into();
    }
    wardian_core::schedule::validate_schedule_definition(&definition).map_err(CliError::generic)?;
    Ok(definition)
}

pub(crate) fn schedule_definition_args_set(args: &ScheduleDefinitionArgs) -> bool {
    args.every.is_some()
        || args.daily.is_some()
        || args.weekly.is_some()
        || args.monthly.is_some()
        || args.specific_dates.is_some()
        || args.at.is_some()
}

fn apply_end_condition(
    args: &ScheduleDefinitionArgs,
    definition: &mut ScheduleDefinition,
) -> Result<(), CliError> {
    if args.end.is_none() && (args.end_date.is_some() || args.max_occurrences.is_some()) {
        return Err(CliError::generic(
            "--end-date and --max-occurrences require --end",
        ));
    }
    let Some(end) = args.end.as_deref() else {
        return Ok(());
    };
    match end {
        "never" => {
            definition.end_condition = "never".into();
            definition.end_date = None;
            definition.max_occurrences = None;
        }
        "on_date" => {
            let end_date = args
                .end_date
                .clone()
                .ok_or_else(|| CliError::generic("--end on_date requires --end-date"))?;
            definition.end_condition = "on_date".into();
            definition.end_date = Some(end_date);
            definition.max_occurrences = None;
        }
        "after_occurrences" => {
            let max_occurrences = args.max_occurrences.ok_or_else(|| {
                CliError::generic("--end after_occurrences requires --max-occurrences")
            })?;
            definition.end_condition = "after_occurrences".into();
            definition.end_date = None;
            definition.max_occurrences = Some(max_occurrences);
        }
        other => {
            return Err(CliError::generic(format!(
                "invalid --end `{other}`; expected never, on_date, or after_occurrences"
            )));
        }
    }
    Ok(())
}

pub(crate) fn parse_automation_bindings(
    bind: &[String],
) -> Result<HashMap<String, String>, CliError> {
    let mut bindings = HashMap::new();
    for entry in bind {
        let Some((name, provider)) = entry.split_once('=') else {
            return Err(CliError::generic(format!(
                "invalid --bind `{entry}`; expected name=provider"
            )));
        };
        let name = name.trim();
        let provider = provider.trim();
        if name.is_empty() || provider.is_empty() {
            return Err(CliError::generic(format!(
                "invalid --bind `{entry}`; expected non-empty name=provider"
            )));
        }
        bindings.insert(name.to_string(), provider.to_string());
    }
    Ok(bindings)
}

pub(crate) fn validate_schedule_provider(provider: Option<&str>) -> Result<(), CliError> {
    if let Some(provider) = provider {
        if !wardian_core::automation::assignment::is_known_provider(provider) {
            return Err(CliError::generic(format!(
                "unsupported provider `{provider}`"
            )));
        }
    }
    Ok(())
}

pub(crate) fn validate_schedule_blueprint(
    blueprint_id: &str,
) -> Result<wardian_core::automation::Blueprint, CliError> {
    let path = wardian_core::automation::resolve_blueprint_path(blueprint_id).ok_or_else(|| {
        CliError::generic(format!(
            "blueprint not found in library/automations: {blueprint_id}"
        ))
    })?;
    let blueprint = wardian_core::automation::parse_file(&path).map_err(|error| {
        CliError::generic(format!("could not parse blueprint {blueprint_id}: {error}"))
    })?;
    let report = wardian_core::automation::validate(&blueprint);
    if !report.is_valid() {
        let diagnostics = serde_json::to_string(&report.diagnostics)
            .map_err(|error| CliError::generic(error.to_string()))?;
        return Err(CliError::generic(format!(
            "blueprint {blueprint_id} is invalid: {diagnostics}"
        )));
    }
    Ok(blueprint)
}
