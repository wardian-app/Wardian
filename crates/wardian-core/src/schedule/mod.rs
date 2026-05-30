//! Pure scheduling math + persistence for v2 schedule invokers. No Tauri, no app state.

use crate::models::ScheduleDefinition;
use chrono::{Datelike, TimeZone};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ScheduleDefinition;

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
}
