//! Pure scheduling math + persistence for automation schedule invokers. No Tauri, no app state.

use crate::models::{AutomationAssignments, ScheduleDefinition};
use chrono::{Datelike, TimeZone};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use crate::models::AutomationSchedule;

/// Largest supported weekly recurrence interval.
///
/// Weekly scheduling currently searches forward by day, so keeping this
/// bounded prevents malformed or extreme persisted values from overflowing
/// the search window or monopolizing a scheduler tick. 520 weeks is roughly
/// ten years, which covers normal calendar scheduling while keeping the
/// projection bounded.
pub const MAX_WEEKLY_REPEAT_EVERY: u32 = 520;

fn local_datetime_at(now_ms: u64) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::Local
        .timestamp_millis_opt(i64::try_from(now_ms).ok()?)
        .single()
}

/// Next future fire time in epoch-ms for `schedule`, or `None` if the schedule
/// can never fire again. Missed slots are skipped.
pub fn compute_next_run(schedule: &ScheduleDefinition, now_ms: u64) -> Option<u64> {
    compute_next_run_with_boundary(schedule, now_ms, false)
}

fn compute_next_run_at_or_after(schedule: &ScheduleDefinition, cutoff_ms: u64) -> Option<u64> {
    compute_next_run_with_boundary(schedule, cutoff_ms, true)
}

fn compute_next_run_with_boundary(
    schedule: &ScheduleDefinition,
    now_ms: u64,
    inclusive: bool,
) -> Option<u64> {
    let follows_boundary = |candidate_ms| {
        if inclusive {
            candidate_ms >= now_ms
        } else {
            candidate_ms > now_ms
        }
    };

    match schedule.schedule_type.as_str() {
        "interval" => {
            let mins = schedule.interval_minutes.unwrap_or(0) as u64;
            if mins > 0 {
                Some(now_ms + mins * 60_000)
            } else {
                None
            }
        }
        "daily" => {
            let time_str = schedule.time_of_day.as_deref().unwrap_or("00:00");
            let parts: Vec<&str> = time_str.split(':').collect();
            if parts.len() != 2 {
                return None;
            }
            let hour: u32 = parts[0].parse().unwrap_or(0);
            let minute: u32 = parts[1].parse().unwrap_or(0);

            let now_local = local_datetime_at(now_ms)?;
            let today = now_local.date_naive();
            let target_time = chrono::NaiveTime::from_hms_opt(hour, minute, 0)?;
            let target_naive = today.and_time(target_time);
            let target_local = chrono::Local
                .from_local_datetime(&target_naive)
                .earliest()?;

            let target_ms = target_local.timestamp_millis() as u64;
            if follows_boundary(target_ms) {
                Some(target_ms)
            } else {
                Some(target_ms + 86_400_000)
            }
        }
        "weekly" => {
            let time_str = schedule.time_of_day.as_deref().unwrap_or("00:00");
            let time_parts: Vec<&str> = time_str.split(':').collect();
            if time_parts.len() != 2 {
                return None;
            }
            let hour: u32 = time_parts[0].parse().unwrap_or(0);
            let minute: u32 = time_parts[1].parse().unwrap_or(0);

            let day_names = match &schedule.days_of_week {
                Some(d) if !d.is_empty() => d.clone(),
                _ => return None,
            };

            let day_map = |name: &str| -> Option<chrono::Weekday> {
                match name.to_lowercase().as_str() {
                    "mon" => Some(chrono::Weekday::Mon),
                    "tue" => Some(chrono::Weekday::Tue),
                    "wed" => Some(chrono::Weekday::Wed),
                    "thu" => Some(chrono::Weekday::Thu),
                    "fri" => Some(chrono::Weekday::Fri),
                    "sat" => Some(chrono::Weekday::Sat),
                    "sun" => Some(chrono::Weekday::Sun),
                    _ => None,
                }
            };

            if schedule.repeat_every > MAX_WEEKLY_REPEAT_EVERY {
                return None;
            }
            let repeat_weeks = schedule.repeat_every.max(1) as i64;
            let now_local = local_datetime_at(now_ms)?;
            let mut best: Option<u64> = None;

            let search_days = repeat_weeks.checked_mul(7)?.checked_add(7)? as u32;
            for day_name in &day_names {
                if let Some(target_day) = day_map(day_name) {
                    for offset in 0..search_days {
                        let candidate_date =
                            (now_local + chrono::Duration::days(offset as i64)).date_naive();
                        if candidate_date.weekday() == target_day {
                            if repeat_weeks > 1 {
                                let epoch = chrono::NaiveDate::from_ymd_opt(2000, 1, 3).unwrap();
                                let weeks_since = (candidate_date - epoch).num_weeks();
                                if weeks_since.rem_euclid(repeat_weeks) != 0 {
                                    continue;
                                }
                            }
                            let target_time = chrono::NaiveTime::from_hms_opt(hour, minute, 0)?;
                            let candidate_naive = candidate_date.and_time(target_time);
                            if let Some(candidate_local) = chrono::Local
                                .from_local_datetime(&candidate_naive)
                                .earliest()
                            {
                                let candidate_ms = candidate_local.timestamp_millis() as u64;
                                if follows_boundary(candidate_ms) {
                                    best = Some(
                                        best.map_or(candidate_ms, |b: u64| b.min(candidate_ms)),
                                    );
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            best
        }
        "monthly" => {
            let time_str = schedule.time_of_day.as_deref().unwrap_or("00:00");
            let time_parts: Vec<&str> = time_str.split(':').collect();
            if time_parts.len() != 2 {
                return None;
            }
            let hour: u32 = time_parts[0].parse().unwrap_or(0);
            let minute: u32 = time_parts[1].parse().unwrap_or(0);

            let target_days = match &schedule.days_of_month {
                Some(d) if !d.is_empty() => d.clone(),
                _ => return None,
            };

            let now_local = local_datetime_at(now_ms)?;
            let target_time = chrono::NaiveTime::from_hms_opt(hour, minute, 0)?;
            let mut best: Option<u64> = None;

            for month_offset in 0..3i32 {
                let candidate_month = now_local.month() as i32 + month_offset;
                let candidate_year = now_local.year() + (candidate_month - 1) / 12;
                let candidate_month_norm = ((candidate_month - 1) % 12 + 1) as u32;

                for &day in &target_days {
                    if let Some(candidate_date) =
                        chrono::NaiveDate::from_ymd_opt(candidate_year, candidate_month_norm, day)
                    {
                        let candidate_naive = candidate_date.and_time(target_time);
                        if let Some(candidate_local) = chrono::Local
                            .from_local_datetime(&candidate_naive)
                            .earliest()
                        {
                            let candidate_ms = candidate_local.timestamp_millis() as u64;
                            if follows_boundary(candidate_ms) {
                                best =
                                    Some(best.map_or(candidate_ms, |b: u64| b.min(candidate_ms)));
                            }
                        }
                    }
                }

                if best.is_some() {
                    break;
                }
            }

            best
        }
        "specific_dates" => {
            let time_str = schedule.time_of_day.as_deref().unwrap_or("00:00");
            let time_parts: Vec<&str> = time_str.split(':').collect();
            let hour: u32 = time_parts.first().and_then(|p| p.parse().ok()).unwrap_or(0);
            let minute: u32 = time_parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(0);

            let dates = match &schedule.specific_dates {
                Some(d) if !d.is_empty() => d.clone(),
                _ => return None,
            };

            let target_time = chrono::NaiveTime::from_hms_opt(hour, minute, 0)?;
            let mut best: Option<u64> = None;

            for date_str in &dates {
                if let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                    let candidate_naive = date.and_time(target_time);
                    if let Some(candidate_local) = chrono::Local
                        .from_local_datetime(&candidate_naive)
                        .earliest()
                    {
                        let candidate_ms = candidate_local.timestamp_millis() as u64;
                        if follows_boundary(candidate_ms) {
                            best = Some(best.map_or(candidate_ms, |b: u64| b.min(candidate_ms)));
                        }
                    }
                }
            }

            best
        }
        "one_time" => {
            let run_at = schedule.run_at.as_deref().unwrap_or("");
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(run_at) {
                let ms = dt.timestamp_millis() as u64;
                if ms > now_ms {
                    Some(ms)
                } else {
                    None
                }
            } else if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(run_at, "%Y-%m-%dT%H:%M") {
                let local = chrono::Local.from_local_datetime(&dt).earliest()?;
                let ms = local.timestamp_millis() as u64;
                if ms > now_ms {
                    Some(ms)
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Validate a persisted schedule definition before it is handed to the scheduler.
///
/// The UI and CLI both construct this DTO, so this check deliberately validates
/// the serialized fields rather than relying on a particular caller's controls.
pub fn validate_schedule_definition(schedule: &ScheduleDefinition) -> Result<(), String> {
    match schedule.schedule_type.as_str() {
        "interval" => {
            if schedule.interval_minutes.unwrap_or(0) == 0 {
                return Err("interval schedules require --every greater than zero".to_string());
            }
        }
        "daily" => {
            validate_time_of_day(schedule.time_of_day.as_deref())?;
        }
        "weekly" => {
            validate_time_of_day(schedule.time_of_day.as_deref())?;
            let days = schedule
                .days_of_week
                .as_ref()
                .filter(|days| !days.is_empty())
                .ok_or_else(|| "weekly schedules require at least one day".to_string())?;
            for day in days {
                if !matches!(
                    day.to_ascii_lowercase().as_str(),
                    "mon" | "tue" | "wed" | "thu" | "fri" | "sat" | "sun"
                ) {
                    return Err(format!("invalid weekly day `{day}`"));
                }
            }
            if schedule.repeat_every == 0 {
                return Err("weekly schedules require repeat_every greater than zero".to_string());
            }
            if schedule.repeat_every > MAX_WEEKLY_REPEAT_EVERY {
                return Err(format!(
                    "weekly schedules require repeat_every no greater than {MAX_WEEKLY_REPEAT_EVERY}"
                ));
            }
        }
        "monthly" => {
            validate_time_of_day(schedule.time_of_day.as_deref())?;
            let days = schedule
                .days_of_month
                .as_ref()
                .filter(|days| !days.is_empty())
                .ok_or_else(|| "monthly schedules require at least one day".to_string())?;
            if let Some(day) = days.iter().find(|day| **day == 0 || **day > 31) {
                return Err(format!("invalid monthly day `{day}`; expected 1-31"));
            }
        }
        "specific_dates" => {
            validate_time_of_day(schedule.time_of_day.as_deref())?;
            let dates = schedule
                .specific_dates
                .as_ref()
                .filter(|dates| !dates.is_empty())
                .ok_or_else(|| "specific_dates schedules require at least one date".to_string())?;
            for date in dates {
                chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                    .map_err(|_| format!("invalid specific date `{date}`; expected YYYY-MM-DD"))?;
            }
        }
        "one_time" => {
            let run_at = schedule
                .run_at
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "one_time schedules require a run_at value".to_string())?;
            if chrono::DateTime::parse_from_rfc3339(run_at).is_err()
                && chrono::NaiveDateTime::parse_from_str(run_at, "%Y-%m-%dT%H:%M").is_err()
            {
                return Err(format!(
                    "invalid run_at `{run_at}`; expected RFC3339 or YYYY-MM-DDTHH:MM"
                ));
            }
        }
        other => {
            return Err(format!(
                "unsupported schedule type `{other}`; expected interval, daily, weekly, monthly, specific_dates, or one_time"
            ));
        }
    }

    match schedule.end_condition.as_str() {
        "never" => {}
        "on_date" => {
            let end_date = schedule
                .end_date
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "on_date schedules require end_date".to_string())?;
            chrono::NaiveDate::parse_from_str(end_date, "%Y-%m-%d")
                .map_err(|_| format!("invalid end_date `{end_date}`; expected YYYY-MM-DD"))?;
        }
        "after_occurrences" => {
            if schedule.max_occurrences.unwrap_or(0) == 0 {
                return Err(
                    "after_occurrences schedules require max_occurrences greater than zero"
                        .to_string(),
                );
            }
        }
        other => {
            return Err(format!(
                "unsupported end condition `{other}`; expected never, on_date, or after_occurrences"
            ));
        }
    }

    Ok(())
}

fn validate_time_of_day(time: Option<&str>) -> Result<(), String> {
    let time = time
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "schedule requires a time_of_day value".to_string())?;
    let mut parts = time.split(':');
    let hour = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .ok_or_else(|| format!("invalid time_of_day `{time}`; expected HH:MM"))?;
    let minute = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .ok_or_else(|| format!("invalid time_of_day `{time}`; expected HH:MM"))?;
    if parts.next().is_some() || hour > 23 || minute > 59 {
        return Err(format!("invalid time_of_day `{time}`; expected HH:MM"));
    }
    Ok(())
}

/// Resolve and validate a schedule workspace to an absolute existing directory.
pub fn resolve_workspace_path(value: &str) -> Result<PathBuf, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("workspace is required".to_string());
    }
    let raw = Path::new(trimmed);
    let absolute = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("could not resolve current directory: {error}"))?
            .join(raw)
    };
    let canonical = absolute
        .canonicalize()
        .map_err(|error| format!("workspace is not an existing directory: {error}"))?;
    #[cfg(windows)]
    let canonical = {
        let value = canonical.to_string_lossy();
        value
            .strip_prefix(r"\\?\")
            .map(PathBuf::from)
            .unwrap_or(canonical)
    };
    if !canonical.is_dir() {
        return Err(format!(
            "workspace is not a directory: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

#[derive(Serialize, Deserialize)]
struct ScheduleFile {
    #[serde(default = "default_schema")]
    schema: u8,
    #[serde(default)]
    schedules: Vec<AutomationSchedule>,
}

fn default_schema() -> u8 {
    1
}

/// Read all schedules while preserving storage and parse failures for callers
/// that cannot safely treat unavailable authoritative state as empty.
pub fn try_load_schedules() -> Result<Vec<AutomationSchedule>, String> {
    let Some(path) = crate::paths::schedules_path() else {
        return Err("no wardian home".to_string());
    };
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    serde_json::from_str::<ScheduleFile>(&content)
        .map(|file| file.schedules)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

/// Read all schedules. Missing or malformed file -> empty (logged to stderr), never panics.
pub fn load_schedules() -> Vec<AutomationSchedule> {
    match try_load_schedules() {
        Ok(schedules) => schedules,
        Err(error) => {
            eprintln!("[wardian-core] {error}");
            Vec::new()
        }
    }
}

/// Serialize one schedule read-modify-write across the app and CLI processes.
///
/// The changed flag keeps scheduler ticks from rewriting an unchanged file.
/// A malformed existing file fails closed so no mutation can silently replace
/// the operator's schedule set with an empty one.
pub fn mutate_schedules<T>(
    mutate: impl FnOnce(&mut Vec<AutomationSchedule>) -> std::io::Result<(T, bool)>,
) -> std::io::Result<T> {
    let path = crate::paths::schedules_path().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "Wardian home is unavailable")
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path.with_extension("lock"))?;
    FileExt::lock_exclusive(&lock)?;

    let mut schedules = read_schedules_for_mutation(&path)?;
    let (result, changed) = mutate(&mut schedules)?;
    if changed {
        write_schedules_to_path(&path, &schedules)?;
    }
    Ok(result)
}

fn read_schedules_for_mutation(path: &std::path::Path) -> std::io::Result<Vec<AutomationSchedule>> {
    match std::fs::read_to_string(path) {
        Ok(body) => serde_json::from_str::<ScheduleFile>(&body)
            .map(|file| file.schedules)
            .map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "refusing to overwrite malformed {}: {error}",
                        path.display()
                    ),
                )
            }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn write_schedules_to_path(
    path: &std::path::Path,
    schedules: &[AutomationSchedule],
) -> std::io::Result<()> {
    let file = ScheduleFile {
        schema: 1,
        schedules: schedules.to_vec(),
    };
    let body = serde_json::to_string_pretty(&file)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// What the effect layer must launch for one firing schedule.
#[derive(Debug, Clone, PartialEq)]
pub struct FireRequest {
    pub schedule_id: String,
    pub blueprint_id: String,
    pub name: String,
    pub provider: Option<String>,
    pub workspace: Option<String>,
    pub input: serde_json::Value,
    pub bindings: HashMap<String, String>,
    pub assignments: AutomationAssignments,
}

fn is_expired(schedule: &AutomationSchedule, now_ms: u64) -> bool {
    match schedule.schedule.end_condition.as_str() {
        "after_occurrences" => schedule
            .schedule
            .max_occurrences
            .is_some_and(|max| schedule.schedule.occurrence_count >= max),
        "on_date" => schedule.schedule.end_date.as_ref().is_some_and(|date| {
            let Some(now) = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now_ms as i64)
            else {
                return false;
            };
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .map(|end| now.with_timezone(&chrono::Local).date_naive() > end)
                .unwrap_or(false)
        }),
        _ => false,
    }
}

fn candidate_within_end_condition(schedule: &ScheduleDefinition, candidate_ms: u64) -> bool {
    if schedule.end_condition != "on_date" {
        return true;
    }
    let Some(end_date) = schedule
        .end_date
        .as_deref()
        .and_then(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
    else {
        return false;
    };
    local_datetime_at(candidate_ms).is_some_and(|candidate| candidate.date_naive() <= end_date)
}

fn next_startup_occurrence(schedule: &ScheduleDefinition, cutoff_ms: u64) -> Option<u64> {
    compute_next_run_at_or_after(schedule, cutoff_ms)
        .filter(|candidate| candidate_within_end_condition(schedule, *candidate))
}

fn fire_request(schedule: &AutomationSchedule) -> FireRequest {
    FireRequest {
        schedule_id: schedule.id.clone(),
        blueprint_id: schedule.blueprint_id.clone(),
        name: schedule.name.clone(),
        provider: schedule.provider.clone(),
        workspace: schedule.workspace.clone(),
        input: schedule.input.clone(),
        bindings: schedule.bindings.clone(),
        assignments: schedule.assignments.clone(),
    }
}

fn advance_after_fire(schedule: &mut AutomationSchedule, now_ms: u64) -> bool {
    schedule.schedule.occurrence_count = schedule.schedule.occurrence_count.saturating_add(1);
    schedule.last_run_status = Some("running".to_string());
    schedule.last_run_error = None;
    schedule.last_run_epoch_ms = Some(now_ms);

    if schedule.schedule.schedule_type == "one_time" || is_expired(schedule, now_ms) {
        return true;
    }

    schedule.next_run_epoch_ms = compute_next_run(&schedule.schedule, now_ms)
        .filter(|candidate| candidate_within_end_condition(&schedule.schedule, *candidate));
    schedule.next_run_epoch_ms.is_none() && schedule.schedule.schedule_type == "specific_dates"
}

/// Reconcile overdue schedules once when the live scheduler activates.
///
/// Occurrences strictly before `startup_cutoff_ms` elapsed while the app was
/// offline. Recurring schedules move to their next future occurrence. Missed
/// one-time and exhausted specific-date schedules remain available for manual
/// Run now.
/// This is deliberately separate from `plan_tick`, so ordinary delayed ticks
/// still dispatch their due work.
pub fn reconcile_startup_schedules(
    schedules: &mut Vec<AutomationSchedule>,
    startup_cutoff_ms: u64,
) -> bool {
    let mut changed = false;
    let mut remove_ids = Vec::new();

    for schedule in schedules.iter_mut() {
        if !schedule.schedule.active || schedule.is_paused {
            continue;
        }

        if is_expired(schedule, startup_cutoff_ms) {
            remove_ids.push(schedule.id.clone());
            continue;
        }

        let Some(next_run) = schedule.next_run_epoch_ms else {
            continue;
        };
        if next_run >= startup_cutoff_ms {
            continue;
        }

        let next_run = if schedule.schedule.schedule_type == "one_time" {
            None
        } else {
            next_startup_occurrence(&schedule.schedule, startup_cutoff_ms)
        };
        let missed_once = schedule.schedule.schedule_type == "one_time"
            || (schedule.schedule.schedule_type == "specific_dates" && next_run.is_none());
        if schedule.next_run_epoch_ms != next_run {
            schedule.next_run_epoch_ms = next_run;
            changed = true;
        }
        if missed_once && schedule.last_run_status.as_deref() != Some("missed") {
            // Keep actual run time, error, and occurrence count as run history.
            schedule.last_run_status = Some("missed".to_string());
            changed = true;
        }
    }

    if !remove_ids.is_empty() {
        schedules.retain(|schedule| !remove_ids.iter().any(|id| id == &schedule.id));
        changed = true;
    }

    changed
}

/// Advance one tick. Mutates `schedules` and returns due fire requests.
pub fn plan_tick(schedules: &mut Vec<AutomationSchedule>, now_ms: u64) -> Vec<FireRequest> {
    let mut fire_requests = Vec::new();
    let mut remove_ids = Vec::new();

    for schedule in schedules.iter_mut() {
        if !schedule.schedule.active || schedule.is_paused {
            continue;
        }

        if is_expired(schedule, now_ms) {
            remove_ids.push(schedule.id.clone());
            continue;
        }

        let Some(next_run) = schedule.next_run_epoch_ms else {
            schedule.next_run_epoch_ms = compute_next_run(&schedule.schedule, now_ms)
                .filter(|candidate| candidate_within_end_condition(&schedule.schedule, *candidate));
            continue;
        };

        if next_run > now_ms {
            continue;
        }

        fire_requests.push(fire_request(schedule));
        if advance_after_fire(schedule, now_ms) {
            remove_ids.push(schedule.id.clone());
        }
    }

    if !remove_ids.is_empty() {
        schedules.retain(|schedule| !remove_ids.iter().any(|id| id == &schedule.id));
    }

    fire_requests
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ScheduleDefinition;

    struct TestHome {
        _guard: std::sync::MutexGuard<'static, ()>,
        _home: tempfile::TempDir,
        previous: Option<std::ffi::OsString>,
    }

    impl TestHome {
        fn new() -> Self {
            let guard = crate::tests::env_lock();
            let home = tempfile::tempdir().expect("temp home");
            let previous = std::env::var_os("WARDIAN_HOME");
            std::env::set_var("WARDIAN_HOME", home.path());
            Self {
                _guard: guard,
                _home: home,
                previous,
            }
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var("WARDIAN_HOME", value),
                None => std::env::remove_var("WARDIAN_HOME"),
            }
        }
    }

    fn sample_schedule(id: &str) -> crate::models::AutomationSchedule {
        crate::models::AutomationSchedule {
            id: id.into(),
            blueprint_id: "heartbeat".into(),
            name: "Heartbeat".into(),
            provider: None,
            workspace: None,
            input: serde_json::json!({}),
            bindings: std::collections::HashMap::new(),
            assignments: std::collections::HashMap::new(),
            schedule: ScheduleDefinition {
                schedule_type: "interval".into(),
                interval_minutes: Some(60),
                active: true,
                ..Default::default()
            },
            next_run_epoch_ms: None,
            paused_remaining_ms: None,
            is_paused: false,
            last_run_status: None,
            last_run_error: None,
            last_run_epoch_ms: None,
        }
    }

    fn s_vec(s: &crate::models::AutomationSchedule) -> Vec<crate::models::AutomationSchedule> {
        vec![s.clone()]
    }

    fn interval(mins: u32) -> ScheduleDefinition {
        ScheduleDefinition {
            schedule_type: "interval".into(),
            interval_minutes: Some(mins),
            active: true,
            ..Default::default()
        }
    }

    fn startup_transaction_then_operator_mutation(
        operator_mutation: impl FnOnce(&mut Vec<crate::models::AutomationSchedule>) + Send + 'static,
    ) -> Vec<crate::models::AutomationSchedule> {
        let _home = TestHome::new();
        let cutoff = 1_800_000_000_000_u64;
        let mut overdue = sample_schedule("startup-race");
        overdue.next_run_epoch_ms = Some(cutoff - 1);
        mutate_schedules(|schedules| {
            schedules.push(overdue);
            Ok(((), true))
        })
        .expect("seed schedule");

        let startup_has_reconciled = std::sync::Arc::new(std::sync::Barrier::new(2));
        let release_startup = std::sync::Arc::new(std::sync::Barrier::new(2));
        let operator_is_ready = std::sync::Arc::new(std::sync::Barrier::new(2));

        let startup_ready = startup_has_reconciled.clone();
        let startup_release = release_startup.clone();
        let startup = std::thread::spawn(move || {
            mutate_schedules(|schedules| {
                let changed = reconcile_startup_schedules(schedules, cutoff);
                startup_ready.wait();
                startup_release.wait();
                Ok(((), changed))
            })
            .expect("startup transaction");
        });

        let operator_ready = operator_is_ready.clone();
        let operator = std::thread::spawn(move || {
            operator_ready.wait();
            mutate_schedules(|schedules| {
                operator_mutation(schedules);
                Ok(((), true))
            })
            .expect("operator transaction");
        });

        startup_has_reconciled.wait();
        operator_is_ready.wait();
        release_startup.wait();
        startup.join().expect("join startup writer");
        operator.join().expect("join operator writer");
        try_load_schedules().expect("read final schedules")
    }

    #[test]
    fn startup_reconciliation_serializes_with_pause() {
        let schedules = startup_transaction_then_operator_mutation(|schedules| {
            let schedule = schedules
                .iter_mut()
                .find(|schedule| schedule.id == "startup-race")
                .expect("startup schedule");
            schedule.is_paused = true;
            schedule.paused_remaining_ms = schedule.next_run_epoch_ms;
            schedule.next_run_epoch_ms = None;
        });
        assert!(schedules[0].is_paused);
        assert_eq!(schedules[0].next_run_epoch_ms, None);
    }

    #[test]
    fn startup_reconciliation_serializes_with_remove() {
        let schedules = startup_transaction_then_operator_mutation(|schedules| {
            schedules.retain(|schedule| schedule.id != "startup-race");
        });
        assert!(schedules.is_empty());
    }

    #[test]
    fn startup_reconciliation_serializes_with_input_and_assignments_update() {
        let schedules = startup_transaction_then_operator_mutation(|schedules| {
            let schedule = schedules
                .iter_mut()
                .find(|schedule| schedule.id == "startup-race")
                .expect("startup schedule");
            schedule.input = serde_json::json!({"release": "operator-edit"});
            schedule.assignments.insert(
                "reviewer".into(),
                crate::models::AutomationRoleAssignment::TemporaryProvider {
                    provider: "gemini".into(),
                    workspace: Some("/review".into()),
                    model: Some("gemini-pro".into()),
                    effort: Some("high".into()),
                },
            );
        });
        assert_eq!(
            schedules[0].input,
            serde_json::json!({"release": "operator-edit"})
        );
        assert!(schedules[0].assignments.contains_key("reviewer"));
    }

    #[test]
    fn malformed_schedule_storage_is_never_overwritten_by_mutation() {
        let _home = TestHome::new();
        let path = crate::paths::schedules_path().expect("schedule path");
        std::fs::create_dir_all(path.parent().unwrap()).expect("schedule directory");
        let malformed = b"{ this is not schedule JSON";
        std::fs::write(&path, malformed).expect("write malformed fixture");

        let error = mutate_schedules(|schedules| {
            schedules.push(sample_schedule("must-not-replace"));
            Ok(((), true))
        })
        .expect_err("malformed storage must fail closed");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read(path).unwrap(), malformed);
    }

    #[test]
    fn interval_projects_forward_from_now() {
        let next = compute_next_run(&interval(5), 1_000_000).unwrap();
        assert_eq!(next, 1_000_000 + 5 * 60_000);
    }

    #[test]
    fn skip_missed_never_returns_past_slot() {
        let next = compute_next_run(&interval(60), 10_000_000_000).unwrap();
        assert!(next > 10_000_000_000);
    }

    #[test]
    fn interval_zero_is_inert() {
        assert!(compute_next_run(&interval(0), 0).is_none());
    }

    #[test]
    fn compute_next_run_weekly_epoch_alignment() {
        use chrono::TimeZone;
        let schedule = ScheduleDefinition {
            schedule_type: "weekly".to_string(),
            time_of_day: Some("09:00".to_string()),
            days_of_week: Some(vec!["Mon".to_string(), "Wed".to_string()]),
            repeat_every: 2,
            ..Default::default()
        };
        let now = chrono::NaiveDate::from_ymd_opt(2024, 1, 1)
            .unwrap()
            .and_time(chrono::NaiveTime::from_hms_opt(8, 0, 0).unwrap());
        let now_ms = chrono::Local
            .from_local_datetime(&now)
            .earliest()
            .unwrap()
            .timestamp_millis() as u64;

        let next = compute_next_run(&schedule, now_ms);
        assert!(next.is_some());
        assert!(next.unwrap() > now_ms);
    }

    #[test]
    fn weekly_repeat_every_is_bounded_before_search() {
        let schedule = ScheduleDefinition {
            schedule_type: "weekly".to_string(),
            time_of_day: Some("09:00".to_string()),
            days_of_week: Some(vec!["Mon".to_string()]),
            repeat_every: MAX_WEEKLY_REPEAT_EVERY,
            end_condition: "never".to_string(),
            ..Default::default()
        };
        assert!(compute_next_run(&schedule, 0).is_some());
        validate_schedule_definition(&schedule).unwrap();

        let out_of_bounds = ScheduleDefinition {
            repeat_every: MAX_WEEKLY_REPEAT_EVERY + 1,
            ..schedule
        };
        assert!(compute_next_run(&out_of_bounds, 0).is_none());
        let error = validate_schedule_definition(&out_of_bounds).unwrap_err();
        assert!(error.contains("no greater than 520"));
    }

    #[test]
    fn startup_reconciliation_skips_offline_occurrences_without_recording_runs() {
        use chrono::TimeZone;

        let cutoff = 1_800_000_000_000_u64;
        let cutoff_local = chrono::Local
            .timestamp_millis_opt(cutoff as i64)
            .single()
            .unwrap();
        let missed_date = cutoff_local.date_naive().pred_opt().unwrap();
        let future_date = cutoff_local.date_naive().succ_opt().unwrap();
        let past_date_run = chrono::Local
            .from_local_datetime(
                &missed_date
                    .and_hms_opt(9, 0, 0)
                    .expect("valid missed local time"),
            )
            .earliest()
            .unwrap()
            .timestamp_millis() as u64;

        let mut recurring = sample_schedule("recurring");
        recurring.next_run_epoch_ms = Some(cutoff - 1);
        recurring.schedule.occurrence_count = 2;
        recurring.last_run_status = Some("completed".into());
        recurring.last_run_epoch_ms = Some(cutoff - 3_600_000);

        let mut one_time = sample_schedule("one-time");
        one_time.schedule.schedule_type = "one_time".into();
        one_time.schedule.run_at = Some("2020-01-01T00:00:00-05:00".into());
        one_time.schedule.end_condition = "after_occurrences".into();
        one_time.schedule.max_occurrences = Some(2);
        one_time.schedule.occurrence_count = 1;
        one_time.next_run_epoch_ms = Some(cutoff - 2);
        one_time.last_run_status = Some("completed".into());
        one_time.last_run_error = Some("prior run detail".into());
        one_time.last_run_epoch_ms = Some(cutoff - 3_600_000);
        one_time.bindings.insert("reviewer".into(), "codex".into());

        let mut specific_dates = sample_schedule("specific-dates");
        specific_dates.schedule.schedule_type = "specific_dates".into();
        specific_dates.schedule.time_of_day = Some("09:00".into());
        specific_dates.schedule.specific_dates = Some(vec![
            missed_date.format("%Y-%m-%d").to_string(),
            future_date.format("%Y-%m-%d").to_string(),
        ]);
        specific_dates.schedule.end_condition = "after_occurrences".into();
        specific_dates.schedule.max_occurrences = Some(10);
        specific_dates.next_run_epoch_ms = Some(past_date_run);
        specific_dates.schedule.occurrence_count = 4;
        specific_dates.last_run_status = Some("completed".into());
        specific_dates.last_run_error = Some("prior specific-date detail".into());
        specific_dates.last_run_epoch_ms = Some(cutoff - 86_400_000);

        let mut exhausted_specific_dates = sample_schedule("exhausted-specific-dates");
        exhausted_specific_dates.schedule.schedule_type = "specific_dates".into();
        exhausted_specific_dates.schedule.time_of_day = Some("09:00".into());
        exhausted_specific_dates.schedule.specific_dates =
            Some(vec![missed_date.format("%Y-%m-%d").to_string()]);
        exhausted_specific_dates.schedule.end_condition = "never".into();
        exhausted_specific_dates.next_run_epoch_ms = Some(past_date_run);
        exhausted_specific_dates.schedule.occurrence_count = 3;
        exhausted_specific_dates.last_run_status = Some("completed".into());
        exhausted_specific_dates.last_run_error = Some("prior exhausted-date detail".into());
        exhausted_specific_dates.last_run_epoch_ms = Some(cutoff - 172_800_000);

        let mut exact_boundary = sample_schedule("exact-boundary");
        exact_boundary.next_run_epoch_ms = Some(cutoff);

        let mut paused = sample_schedule("paused");
        paused.next_run_epoch_ms = Some(cutoff - 1);
        paused.is_paused = true;

        let mut expired = sample_schedule("expired");
        expired.next_run_epoch_ms = Some(cutoff - 1);
        expired.schedule.end_condition = "after_occurrences".into();
        expired.schedule.max_occurrences = Some(2);
        expired.schedule.occurrence_count = 2;

        let mut schedules = vec![
            recurring,
            one_time,
            specific_dates,
            exhausted_specific_dates,
            exact_boundary,
            paused,
            expired,
        ];

        assert!(reconcile_startup_schedules(&mut schedules, cutoff));
        assert_eq!(
            schedules.len(),
            6,
            "expired schedules keep normal expiry behavior"
        );

        let recurring = schedules
            .iter()
            .find(|schedule| schedule.id == "recurring")
            .unwrap();
        assert!(recurring.next_run_epoch_ms.unwrap() > cutoff);
        assert_eq!(recurring.schedule.occurrence_count, 2);
        assert_eq!(recurring.last_run_status.as_deref(), Some("completed"));
        assert_eq!(recurring.last_run_epoch_ms, Some(cutoff - 3_600_000));

        let one_time = schedules
            .iter()
            .find(|schedule| schedule.id == "one-time")
            .unwrap();
        assert_eq!(one_time.last_run_status.as_deref(), Some("missed"));
        assert_eq!(one_time.next_run_epoch_ms, None);
        assert_eq!(one_time.last_run_epoch_ms, Some(cutoff - 3_600_000));
        assert_eq!(one_time.last_run_error.as_deref(), Some("prior run detail"));
        assert_eq!(one_time.schedule.occurrence_count, 1);
        assert_eq!(one_time.schedule.end_condition, "after_occurrences");
        assert_eq!(one_time.schedule.max_occurrences, Some(2));
        assert_eq!(
            one_time.bindings.get("reviewer").map(String::as_str),
            Some("codex")
        );

        let specific_dates = schedules
            .iter()
            .find(|schedule| schedule.id == "specific-dates")
            .unwrap();
        assert_eq!(specific_dates.last_run_status.as_deref(), Some("completed"));
        assert_eq!(
            specific_dates.last_run_error.as_deref(),
            Some("prior specific-date detail")
        );
        assert_eq!(specific_dates.schedule.occurrence_count, 4);
        assert_eq!(specific_dates.schedule.end_condition, "after_occurrences");
        assert_eq!(specific_dates.schedule.max_occurrences, Some(10));
        assert_eq!(specific_dates.last_run_epoch_ms, Some(cutoff - 86_400_000));
        let next_specific_date = specific_dates.next_run_epoch_ms.unwrap();
        assert!(next_specific_date > cutoff);
        assert_eq!(
            chrono::Local
                .timestamp_millis_opt(next_specific_date as i64)
                .single()
                .unwrap()
                .date_naive(),
            future_date,
        );

        let exhausted = schedules
            .iter()
            .find(|schedule| schedule.id == "exhausted-specific-dates")
            .unwrap();
        assert_eq!(exhausted.last_run_status.as_deref(), Some("missed"));
        assert_eq!(
            exhausted.last_run_error.as_deref(),
            Some("prior exhausted-date detail")
        );
        assert_eq!(exhausted.last_run_epoch_ms, Some(cutoff - 172_800_000));
        assert_eq!(exhausted.next_run_epoch_ms, None);
        assert_eq!(exhausted.schedule.occurrence_count, 3);
        assert_eq!(exhausted.schedule.end_condition, "never");

        assert_eq!(
            schedules
                .iter()
                .find(|schedule| schedule.id == "exact-boundary")
                .unwrap()
                .next_run_epoch_ms,
            Some(cutoff),
            "an occurrence exactly at startup remains due",
        );
        assert_eq!(
            schedules
                .iter()
                .find(|schedule| schedule.id == "paused")
                .unwrap()
                .next_run_epoch_ms,
            Some(cutoff - 1),
        );
        assert_eq!(
            plan_tick(
                &mut vec![schedules
                    .iter()
                    .find(|schedule| schedule.id == "exact-boundary")
                    .unwrap()
                    .clone()],
                cutoff + 5_000,
            )
            .len(),
            1,
            "the first delayed active tick still fires the exact-boundary occurrence",
        );

        let timezone_one_time = ScheduleDefinition {
            schedule_type: "one_time".into(),
            run_at: Some("2020-01-01T00:00:00-05:00".into()),
            ..Default::default()
        };
        let expected_epoch = chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00-05:00")
            .unwrap()
            .timestamp_millis() as u64;
        assert_eq!(
            compute_next_run(&timezone_one_time, expected_epoch - 1),
            Some(expected_epoch)
        );

        use crate::models::{AgentConversationMode, AutomationRoleAssignment, BusyPolicy};

        let mut manual_one_time = schedules
            .iter()
            .find(|schedule| schedule.id == "one-time")
            .unwrap()
            .clone();
        manual_one_time.next_run_epoch_ms = Some(cutoff);
        manual_one_time.input = serde_json::json!({"report_date": "2026-10-22", "dry_run": true});
        manual_one_time.provider = Some("claude".into());
        manual_one_time.workspace = Some("/workspace/one-time".into());
        let one_time_assignments = std::collections::HashMap::from([(
            "reviewer".into(),
            AutomationRoleAssignment::TemporaryProvider {
                provider: "gemini".into(),
                workspace: Some("/workspace/reviewer".into()),
                model: Some("gemini-pro".into()),
                effort: Some("high".into()),
            },
        )]);
        manual_one_time.assignments = one_time_assignments.clone();
        let one_time_bindings = manual_one_time.bindings.clone();

        let mut manual_specific_dates = schedules
            .iter()
            .find(|schedule| schedule.id == "exhausted-specific-dates")
            .unwrap()
            .clone();
        manual_specific_dates.next_run_epoch_ms = Some(cutoff);
        manual_specific_dates.input = serde_json::json!({"edition": "winter", "notify": false});
        manual_specific_dates.provider = Some("codex".into());
        manual_specific_dates.workspace = Some("/workspace/specific-date".into());
        let specific_date_assignments = std::collections::HashMap::from([(
            "publisher".into(),
            AutomationRoleAssignment::Agent {
                agent_id: "agent-publisher".into(),
                conversation: AgentConversationMode::FreshBackground,
                busy_policy: BusyPolicy::Queue,
            },
        )]);
        manual_specific_dates.assignments = specific_date_assignments.clone();

        let fires = plan_tick(&mut vec![manual_one_time, manual_specific_dates], cutoff);
        assert_eq!(
            fires.len(),
            2,
            "manual Run now remains available for retained one-off schedules"
        );
        let one_time_fire = fires
            .iter()
            .find(|fire| fire.schedule_id == "one-time")
            .unwrap();
        assert_eq!(
            one_time_fire.input,
            serde_json::json!({"report_date": "2026-10-22", "dry_run": true})
        );
        assert_eq!(one_time_fire.bindings, one_time_bindings);
        assert_eq!(one_time_fire.assignments, one_time_assignments);
        assert_eq!(one_time_fire.provider.as_deref(), Some("claude"));
        assert_eq!(
            one_time_fire.workspace.as_deref(),
            Some("/workspace/one-time")
        );

        let specific_date_fire = fires
            .iter()
            .find(|fire| fire.schedule_id == "exhausted-specific-dates")
            .unwrap();
        assert_eq!(
            specific_date_fire.input,
            serde_json::json!({"edition": "winter", "notify": false})
        );
        assert_eq!(specific_date_fire.assignments, specific_date_assignments);
        assert_eq!(specific_date_fire.provider.as_deref(), Some("codex"));
        assert_eq!(
            specific_date_fire.workspace.as_deref(),
            Some("/workspace/specific-date")
        );
    }

    #[test]
    fn startup_calendar_roll_forward_keeps_occurrences_at_the_cutoff_due() {
        use chrono::TimeZone;

        let cutoff_local = chrono::Local
            .with_ymd_and_hms(2026, 10, 23, 9, 0, 0)
            .single()
            .unwrap();
        let cutoff = cutoff_local.timestamp_millis() as u64;
        let previous_day = cutoff_local.date_naive().pred_opt().unwrap();
        let previous_occurrence = chrono::Local
            .from_local_datetime(&previous_day.and_hms_opt(9, 0, 0).unwrap())
            .earliest()
            .unwrap()
            .timestamp_millis() as u64;

        let mut daily = sample_schedule("daily-at-cutoff");
        daily.schedule = ScheduleDefinition {
            schedule_type: "daily".into(),
            time_of_day: Some("09:00".into()),
            active: true,
            ..Default::default()
        };
        daily.next_run_epoch_ms = Some(previous_occurrence);
        daily.last_run_status = Some("completed".into());
        let mut daily_schedules = vec![daily];
        assert!(reconcile_startup_schedules(&mut daily_schedules, cutoff));
        assert_eq!(daily_schedules[0].next_run_epoch_ms, Some(cutoff));
        assert_eq!(
            daily_schedules[0].last_run_status.as_deref(),
            Some("completed")
        );
        assert_eq!(plan_tick(&mut daily_schedules, cutoff + 5_000).len(), 1);

        let mut specific = sample_schedule("specific-date-at-cutoff");
        specific.schedule = ScheduleDefinition {
            schedule_type: "specific_dates".into(),
            time_of_day: Some("09:00".into()),
            specific_dates: Some(vec![cutoff_local
                .date_naive()
                .format("%Y-%m-%d")
                .to_string()]),
            active: true,
            ..Default::default()
        };
        specific.next_run_epoch_ms = Some(previous_occurrence);
        let mut specific_schedules = vec![specific];
        assert!(reconcile_startup_schedules(&mut specific_schedules, cutoff));
        assert_eq!(specific_schedules[0].next_run_epoch_ms, Some(cutoff));
        assert_eq!(plan_tick(&mut specific_schedules, cutoff + 5_000).len(), 1);
    }

    #[test]
    fn specific_date_startup_projection_respects_end_date_and_keeps_manual_run() {
        use chrono::TimeZone;

        let cutoff_local = chrono::Local
            .with_ymd_and_hms(2026, 10, 23, 12, 0, 0)
            .single()
            .unwrap();
        let cutoff = cutoff_local.timestamp_millis() as u64;
        let candidate_date = cutoff_local
            .date_naive()
            .succ_opt()
            .unwrap()
            .succ_opt()
            .unwrap();
        let previous_date = cutoff_local.date_naive().pred_opt().unwrap();
        let previous_occurrence = chrono::Local
            .from_local_datetime(&previous_date.and_hms_opt(9, 0, 0).unwrap())
            .earliest()
            .unwrap()
            .timestamp_millis() as u64;
        let candidate = chrono::Local
            .from_local_datetime(&candidate_date.and_hms_opt(9, 0, 0).unwrap())
            .earliest()
            .unwrap()
            .timestamp_millis() as u64;

        for (id, end_offset, should_project) in [
            ("before-end", 1_i64, true),
            ("on-end", 0_i64, true),
            ("after-end", -1_i64, false),
        ] {
            let end_date = candidate_date
                .checked_add_signed(chrono::Duration::days(end_offset))
                .unwrap();
            let expected_end_date = end_date.format("%Y-%m-%d").to_string();
            let mut schedule = sample_schedule(id);
            schedule.schedule = ScheduleDefinition {
                schedule_type: "specific_dates".into(),
                time_of_day: Some("09:00".into()),
                specific_dates: Some(vec![candidate_date.format("%Y-%m-%d").to_string()]),
                end_condition: "on_date".into(),
                end_date: Some(expected_end_date.clone()),
                active: true,
                ..Default::default()
            };
            schedule.next_run_epoch_ms = Some(previous_occurrence);
            schedule.last_run_status = Some("completed".into());
            schedule.last_run_error = Some("prior failure detail".into());
            schedule.input = serde_json::json!({"edition": "winter"});

            let mut schedules = vec![schedule];
            assert!(reconcile_startup_schedules(&mut schedules, cutoff));
            let projected = &schedules[0];
            assert_eq!(
                projected.next_run_epoch_ms,
                should_project.then_some(candidate)
            );
            assert_eq!(
                projected.last_run_status.as_deref(),
                Some(if should_project {
                    "completed"
                } else {
                    "missed"
                })
            );
            assert_eq!(
                projected.last_run_error.as_deref(),
                Some("prior failure detail")
            );
            assert_eq!(projected.input, serde_json::json!({"edition": "winter"}));
            assert_eq!(
                projected.schedule.end_date.as_deref(),
                Some(expected_end_date.as_str())
            );

            if !should_project {
                let mut still_missed = vec![projected.clone()];
                assert!(plan_tick(&mut still_missed, cutoff + 5_000).is_empty());
                assert_eq!(still_missed[0].next_run_epoch_ms, None);
                assert_eq!(still_missed[0].last_run_status.as_deref(), Some("missed"));

                let mut manually_due = projected.clone();
                manually_due.next_run_epoch_ms = Some(cutoff);
                let fires = plan_tick(&mut vec![manually_due], cutoff);
                assert_eq!(
                    fires.len(),
                    1,
                    "Run now remains available before expiration"
                );
                assert_eq!(fires[0].input, serde_json::json!({"edition": "winter"}));
            }
        }
    }

    #[test]
    fn calendar_projection_uses_the_supplied_cutoff_in_local_time() {
        use chrono::{Datelike, TimeZone, Timelike};

        let cutoff = chrono::Local
            .with_ymd_and_hms(2020, 5, 2, 8, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64;
        let schedule = ScheduleDefinition {
            schedule_type: "daily".into(),
            time_of_day: Some("09:00".into()),
            ..Default::default()
        };

        let next = compute_next_run(&schedule, cutoff).unwrap();
        let next_local = chrono::Local
            .timestamp_millis_opt(next as i64)
            .single()
            .unwrap();
        assert_eq!(
            next_local.date_naive(),
            chrono::NaiveDate::from_ymd_opt(2020, 5, 2).unwrap()
        );
        assert_eq!((next_local.hour(), next_local.minute()), (9, 0));
        assert_eq!(next_local.weekday(), chrono::Weekday::Sat);
    }

    #[test]
    fn save_then_load_round_trips() {
        let _guard = crate::tests::env_lock();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("WARDIAN_HOME", dir.path());
        let scheds = vec![sample_schedule("s1")];
        mutate_schedules(|schedules| {
            schedules.extend(scheds);
            Ok(((), true))
        })
        .unwrap();
        let loaded = load_schedules();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "s1");
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn load_missing_file_is_empty() {
        let _guard = crate::tests::env_lock();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("WARDIAN_HOME", dir.path());
        assert!(load_schedules().is_empty());
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn strict_schedule_load_preserves_malformed_storage_failure() {
        let _guard = crate::tests::env_lock();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("WARDIAN_HOME", dir.path());
        std::fs::create_dir_all(dir.path().join("library")).unwrap();
        std::fs::write(
            dir.path().join("library").join("schedules.json"),
            "not json",
        )
        .unwrap();

        assert!(try_load_schedules()
            .unwrap_err()
            .contains("failed to parse"));
        assert!(load_schedules().is_empty());
        std::env::remove_var("WARDIAN_HOME");
    }

    #[test]
    fn due_active_schedule_fires_and_advances() {
        let mut s = sample_schedule("s1");
        s.next_run_epoch_ms = Some(500);
        let mut v = s_vec(&s);
        let fires = plan_tick(&mut v, 1000);
        assert_eq!(fires.len(), 1);
        assert_eq!(fires[0].blueprint_id, "heartbeat");
        assert!(v[0].next_run_epoch_ms.is_some_and(|next| next > 1000));
    }

    #[test]
    fn paused_schedule_does_not_fire() {
        let mut s = sample_schedule("s1");
        s.is_paused = true;
        s.next_run_epoch_ms = Some(500);
        let mut v = vec![s];
        assert!(plan_tick(&mut v, 1000).is_empty());
    }

    #[test]
    fn unset_next_run_is_computed_not_fired() {
        let mut v = vec![sample_schedule("s1")];
        let fires = plan_tick(&mut v, 1000);
        assert!(fires.is_empty());
        assert!(v[0].next_run_epoch_ms.is_some());
    }

    #[test]
    fn one_time_is_removed_after_firing() {
        let mut s = sample_schedule("s1");
        s.schedule.schedule_type = "one_time".into();
        s.schedule.interval_minutes = None;
        s.next_run_epoch_ms = Some(500);
        let mut v = vec![s];
        let fires = plan_tick(&mut v, 1000);
        assert_eq!(fires.len(), 1);
        assert!(
            v.is_empty(),
            "one_time schedule should be removed after firing"
        );
    }

    #[test]
    fn after_occurrences_expiry_removes_without_firing() {
        let mut s = sample_schedule("s1");
        s.schedule.end_condition = "after_occurrences".into();
        s.schedule.max_occurrences = Some(2);
        s.schedule.occurrence_count = 2;
        s.next_run_epoch_ms = Some(500);
        let mut v = vec![s];
        let fires = plan_tick(&mut v, 1000);
        assert!(fires.is_empty());
        assert!(v.is_empty());
    }

    #[test]
    fn validates_extended_cadence_and_end_fields() {
        let monthly = ScheduleDefinition {
            schedule_type: "monthly".into(),
            time_of_day: Some("09:30".into()),
            days_of_month: Some(vec![1, 15]),
            end_condition: "after_occurrences".into(),
            max_occurrences: Some(4),
            active: true,
            ..Default::default()
        };
        validate_schedule_definition(&monthly).unwrap();

        let invalid = ScheduleDefinition {
            schedule_type: "weekly".into(),
            time_of_day: Some("25:00".into()),
            days_of_week: Some(vec!["Mon".into()]),
            repeat_every: 1,
            active: true,
            ..Default::default()
        };
        assert!(validate_schedule_definition(&invalid).is_err());
    }

    #[test]
    fn resolves_only_existing_directories_as_workspaces() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-a-directory");
        std::fs::write(&file, "x").unwrap();

        assert!(resolve_workspace_path(&dir.path().to_string_lossy()).is_ok());
        assert!(resolve_workspace_path(&file.to_string_lossy()).is_err());
        assert!(resolve_workspace_path("definitely-missing-workspace").is_err());
    }
}
