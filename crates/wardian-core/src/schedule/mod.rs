//! Pure scheduling math + persistence for v2 schedule invokers. No Tauri, no app state.

use crate::models::ScheduleDefinition;
use chrono::{Datelike, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::models::WorkflowSchedule;

/// Next future fire time in epoch-ms for `schedule`, or `None` if the schedule
/// can never fire again. Missed slots are skipped.
pub fn compute_next_run(schedule: &ScheduleDefinition, now_ms: u64) -> Option<u64> {
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

            let now_local = chrono::Local::now();
            let today = now_local.date_naive();
            let target_time = chrono::NaiveTime::from_hms_opt(hour, minute, 0)?;
            let target_naive = today.and_time(target_time);
            let target_local = chrono::Local.from_local_datetime(&target_naive).earliest()?;

            let target_ms = target_local.timestamp_millis() as u64;
            if target_ms > now_ms {
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

            let repeat_weeks = schedule.repeat_every.max(1) as i64;
            let now_local = chrono::Local::now();
            let mut best: Option<u64> = None;

            let search_days = (repeat_weeks * 7 + 7) as u32;
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
                            if let Some(candidate_local) =
                                chrono::Local.from_local_datetime(&candidate_naive).earliest()
                            {
                                let candidate_ms = candidate_local.timestamp_millis() as u64;
                                if candidate_ms > now_ms {
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

            let now_local = chrono::Local::now();
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
                        if let Some(candidate_local) =
                            chrono::Local.from_local_datetime(&candidate_naive).earliest()
                        {
                            let candidate_ms = candidate_local.timestamp_millis() as u64;
                            if candidate_ms > now_ms {
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
                    if let Some(candidate_local) =
                        chrono::Local.from_local_datetime(&candidate_naive).earliest()
                    {
                        let candidate_ms = candidate_local.timestamp_millis() as u64;
                        if candidate_ms > now_ms {
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
                if ms > now_ms { Some(ms) } else { None }
            } else if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(run_at, "%Y-%m-%dT%H:%M")
            {
                let local = chrono::Local.from_local_datetime(&dt).earliest()?;
                let ms = local.timestamp_millis() as u64;
                if ms > now_ms { Some(ms) } else { None }
            } else {
                None
            }
        }
        _ => None,
    }
}

#[derive(Serialize, Deserialize)]
struct ScheduleFile {
    #[serde(default = "default_schema")]
    schema: u8,
    #[serde(default)]
    schedules: Vec<WorkflowSchedule>,
}

fn default_schema() -> u8 {
    1
}

/// Read all schedules. Missing or malformed file -> empty (logged to stderr), never panics.
pub fn load_schedules() -> Vec<WorkflowSchedule> {
    let Some(path) = crate::paths::schedules_path() else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    match serde_json::from_str::<ScheduleFile>(&content) {
        Ok(file) => file.schedules,
        Err(err) => {
            eprintln!("[wardian-core] malformed schedules.json: {err}");
            Vec::new()
        }
    }
}

/// Write all schedules atomically (temp file + rename) so a crash mid-write cannot truncate.
pub fn save_schedules(schedules: &[WorkflowSchedule]) -> std::io::Result<()> {
    let path = crate::paths::schedules_path()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no wardian home"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = ScheduleFile {
        schema: 1,
        schedules: schedules.to_vec(),
    };
    let body = serde_json::to_string_pretty(&file)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
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
}

fn is_expired(schedule: &WorkflowSchedule, now_ms: u64) -> bool {
    match schedule.schedule.end_condition.as_str() {
        "after_occurrences" => schedule
            .schedule
            .max_occurrences
            .is_some_and(|max| schedule.schedule.occurrence_count >= max),
        "on_date" => schedule.schedule.end_date.as_ref().is_some_and(|date| {
            let Some(now) =
                chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now_ms as i64)
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

fn fire_request(schedule: &WorkflowSchedule) -> FireRequest {
    FireRequest {
        schedule_id: schedule.id.clone(),
        blueprint_id: schedule.blueprint_id.clone(),
        name: schedule.name.clone(),
        provider: schedule.provider.clone(),
        workspace: schedule.workspace.clone(),
        input: schedule.input.clone(),
        bindings: schedule.bindings.clone(),
    }
}

fn advance_after_fire(schedule: &mut WorkflowSchedule, now_ms: u64) -> bool {
    schedule.schedule.occurrence_count = schedule.schedule.occurrence_count.saturating_add(1);
    schedule.last_run_status = Some("running".to_string());
    schedule.last_run_error = None;
    schedule.last_run_epoch_ms = Some(now_ms);

    if schedule.schedule.schedule_type == "one_time" || is_expired(schedule, now_ms) {
        return true;
    }

    schedule.next_run_epoch_ms = compute_next_run(&schedule.schedule, now_ms);
    schedule.next_run_epoch_ms.is_none() && schedule.schedule.schedule_type == "specific_dates"
}

/// Advance one tick. Mutates `schedules` and returns due fire requests.
pub fn plan_tick(schedules: &mut Vec<WorkflowSchedule>, now_ms: u64) -> Vec<FireRequest> {
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
            schedule.next_run_epoch_ms = compute_next_run(&schedule.schedule, now_ms);
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

    fn sample_schedule(id: &str) -> crate::models::WorkflowSchedule {
        crate::models::WorkflowSchedule {
            id: id.into(),
            blueprint_id: "heartbeat".into(),
            name: "Heartbeat".into(),
            provider: None,
            workspace: None,
            input: serde_json::json!({}),
            bindings: std::collections::HashMap::new(),
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

    fn s_vec(s: &crate::models::WorkflowSchedule) -> Vec<crate::models::WorkflowSchedule> {
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
    fn save_then_load_round_trips() {
        let _guard = crate::tests::env_lock();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("WARDIAN_HOME", dir.path());
        let scheds = vec![sample_schedule("s1")];
        save_schedules(&scheds).unwrap();
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
        assert!(v.is_empty(), "one_time schedule should be removed after firing");
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
}
