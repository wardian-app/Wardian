//! Ordinary telemetry collection with an injectable OS sampling boundary.

use super::*;

pub(super) async fn collect_agent_metrics_with_sampler(
    state: &AppState,
    sample_processes: impl FnOnce(
            Arc<tokio::sync::Mutex<sysinfo::System>>,
            Vec<String>,
            Vec<(String, Option<u32>)>,
        ) -> Option<SystemProcessSnapshot>
        + Send
        + 'static,
) -> (Vec<AgentTelemetry>, status::StatusFollowUp) {
    let collect_started = std::time::Instant::now();
    let mut snapshots: Vec<AgentSnapshot> = {
        let agents = state.agents.lock().await;
        let snapshots: Vec<AgentSnapshot> = agents
            .iter()
            .map(|(sid, agent)| {
                let config = agent.config.lock().unwrap();
                AgentSnapshot {
                    session_id: sid.clone(),
                    provider: config.provider.clone(),
                    folder: config.folder.clone(),
                    is_off: config.is_off,
                    resume_session: opencode_telemetry_session_id(&config),
                    conversation_logging: config.conversation_logging,
                    capture_conversation: config
                        .resume_session
                        .clone()
                        .or(config.fresh_provider_session_id.clone()),
                    provider_generation: 0,
                    process_id: agent.process_id,
                    query_count: agent.query_count.clone(),
                    init_timestamp: agent.init_timestamp.clone(),
                    last_query_timestamp: agent.last_query_timestamp.clone(),
                    current_status: agent.current_status.clone(),
                    status_observation: Mutex::new(TelemetryStatusDraft::default()),
                    watch_state: agent.watch_state.clone(),
                    last_output_at: agent.last_output_at.clone(),
                    log_path: agent.log_path.clone(),
                    log_last_modified: agent.log_last_modified.clone(),
                }
            })
            .collect();
        // Prune only against this authoritative roster while it is locked,
        // before a replacement can publish a newer coordinator owner.
        state.background_capture.retain_agents(
            &snapshots
                .iter()
                .map(|snapshot| (snapshot.session_id.clone(), snapshot.current_status.clone()))
                .collect::<Vec<_>>(),
        );
        snapshots
    };
    for snapshot in &snapshots {
        snapshot.capture_initial_status(state);
    }
    let global_logging = crate::utils::shell::load_shell_settings()
        .unwrap_or_default()
        .conversation_logging;
    let capture_observation_sessions = state.background_capture.observation_sessions();
    for snapshot in &mut snapshots {
        snapshot.provider_generation = state
            .interactions
            .current_provider_input_generation(&snapshot.session_id)
            .await
            .unwrap_or(0);
    }

    let pre_pass = collect_started.elapsed();
    let sys_metrics = state.system_metrics.clone();
    let result = tokio::task::spawn_blocking(move || {
        let pass_started = std::time::Instant::now();
        let session_ids = snapshots
            .iter()
            .map(|snap| snap.session_id.clone())
            .collect::<Vec<_>>();
        let active_leases = wardian_core::conversation_lease::load_leases();
        let lease_now = chrono::Utc::now().to_rfc3339();
        let mut results = Vec::new();
        let mut provider_statuses = Vec::new();
        let mut background_captures = Vec::new();
        let mut last_user_query_timestamps = latest_user_query_timestamps();
        let agent_roots = snapshots
            .iter()
            .map(|snap| (snap.session_id.clone(), snap.process_id))
            .collect::<Vec<_>>();
        let system_snapshot = sample_processes(sys_metrics, session_ids, agent_roots);
        let mut slow_agents = Vec::new();
        observe_codex_indexes();
        let loop_started = std::time::Instant::now();
        let (log_total, db_total) = Default::default();

        for snap in &snapshots {
            let agent_started = std::time::Instant::now();
            let mut cpu = 0.0;
            let mut mem = 0.0;
            let mut uptime = 0;
            let mut related_process_ids = BTreeSet::new();

            if let (Some(system_snapshot), Some(pid)) = (&system_snapshot, snap.process_id) {
                #[cfg(windows)]
                let discovered_roots = system_snapshot
                    .session_roots
                    .get(&snap.session_id)
                    .cloned()
                    .unwrap_or_default();
                #[cfg(not(windows))]
                let discovered_roots = Vec::new();

                related_process_ids = collect_related_pids(
                    Some(pid),
                    &discovered_roots,
                    &system_snapshot.children_map,
                );
                let mut raw_cpu = 0.0;
                let mut memory_bytes = 0_u64;
                for pid in &related_process_ids {
                    if let Some(process) = system_snapshot.processes.get(pid) {
                        raw_cpu += process.cpu_usage;
                        memory_bytes = memory_bytes.saturating_add(process.memory);
                        uptime = std::cmp::max(uptime, process.run_time);
                    }
                }
                cpu = normalize_cpu_usage(raw_cpu, system_snapshot.logical_cpu_count);
                mem = bytes_to_mib(memory_bytes);

                // Phase 3: Uptime Alignment
                // If we have a 'Born' date, calculate total lifetime uptime while active.
                // Otherwise, fallback to the OS process runtime gathered above.
                if let Ok(born_lock) = snap.init_timestamp.lock() {
                    if let Some(ref born_str) = *born_lock {
                        if let Ok(born_dt) = chrono::DateTime::parse_from_rfc3339(born_str) {
                            let now = chrono::Utc::now();
                            let duration =
                                now.signed_duration_since(born_dt.with_timezone(&chrono::Utc));
                            let secs = duration.num_seconds();
                            if secs > 0 {
                                uptime = secs as u64;
                            }
                        }
                    }
                }
            }

            // Detect whether the agent process is still alive. If system sampling
            // is skipped, liveness is unknown and must not force a status change.
            let process_alive = system_snapshot.as_ref().map(|system_snapshot| {
                related_process_ids
                    .iter()
                    .any(|pid| system_snapshot.processes.contains_key(pid))
            });

            let mut q_count = *snap.query_count.lock().unwrap();
            let mut i_ts = snap.init_timestamp.lock().unwrap().clone();
            let mut log_path_display = snap
                .log_path
                .try_lock()
                .ok()
                .and_then(|path| path.as_ref().map(|p| display_log_path(p)));
            let opencode_session_id = snap.resume_session.as_deref();
            let gemini_session_id = snap.resume_session.as_deref();
            let status_before_log_work = snap.telemetry_status();
            let mut last_query_timestamp = last_user_query_timestamps.remove(&snap.session_id);
            reconcile_cached_last_query_timestamp(
                &mut last_query_timestamp,
                &snap.last_query_timestamp,
            );
            let run_provider_log_work =
                should_run_provider_log_telemetry(&status_before_log_work, process_alive);

            if run_provider_log_work {
                let _log_clock = PhaseClock::start(&log_total);
                if let Some(_agent_work_guard) = try_begin_agent_telemetry_work(&snap.session_id) {
                    let mut log_path_lock = snap.log_path.lock().unwrap_or_else(|e| e.into_inner());

                    if snap.provider == "gemini" {
                        // Re-verifying the session id requires reading the whole
                        // log, so only do it when the file changed (or vanished)
                        // since the last parse; unchanged content cannot go stale.
                        let last_parsed_mtime =
                            snap.log_last_modified.lock().ok().and_then(|last| *last);
                        let stale_gemini_log = gemini_session_id.is_none()
                            || log_path_lock.as_ref().is_some_and(|path| {
                                let current_mtime = std::fs::metadata(path)
                                    .and_then(|meta| meta.modified())
                                    .ok();
                                match (current_mtime, last_parsed_mtime) {
                                    (Some(current), Some(last)) if current == last => false,
                                    _ => std::fs::read_to_string(path).ok().is_none_or(|content| {
                                        !gemini_log_matches_session(
                                            &content,
                                            gemini_session_id.unwrap_or_default(),
                                        )
                                    }),
                                }
                            });
                        if stale_gemini_log {
                            *log_path_lock = None;
                            if let Ok(mut last_modified) = snap.log_last_modified.lock() {
                                *last_modified = None;
                            }
                        }
                    }

                    // Provider-aware log discovery
                    if snap.provider == "opencode" {
                        let mut discovered_log = None;
                        if let Some(opencode_session_id) = opencode_session_id {
                            for dir in opencode_log_dirs() {
                                if let Some(path) = opencode_log_path_in(&dir, opencode_session_id)
                                {
                                    discovered_log = Some(path);
                                    break;
                                }
                            }
                        }
                        *log_path_lock = discovered_log;
                    } else if snap.provider == "antigravity" {
                        let conversation_id = snap
                            .resume_session
                            .as_ref()
                            .map(|value| value.trim().to_string())
                            .filter(|value| !value.is_empty());
                        if let (Some(home), Some(conversation_id)) =
                            (AntigravityProvider::antigravity_home(), conversation_id)
                        {
                            if let Some(candidate) = AntigravityProvider::conversation_status_path(
                                &home,
                                &conversation_id,
                            ) {
                                *log_path_lock = Some(candidate);
                            }
                        }
                    } else if snap.provider == "claude" && snap.resume_session.is_some() {
                        // For Claude, if we have a resume_session (Conversation ID), always re-verify
                        // the path so it updates immediately after a Clear rotation.
                        if let Some(home) = dirs::home_dir() {
                            let project_dir = claude_project_dir_name(&snap.folder);
                            let session_id_to_find = snap.resume_session.as_deref().unwrap();
                            let candidate = home
                                .join(".claude")
                                .join("projects")
                                .join(&project_dir)
                                .join(format!("{}.jsonl", session_id_to_find));
                            if candidate.exists() {
                                *log_path_lock = Some(candidate);
                            }
                        }
                    } else if snap.provider == "pi" {
                        if let Some(provider_session_id) = snap.resume_session.as_deref() {
                            if let Some(session_dir) = PiProvider::session_dir(&snap.session_id) {
                                if let Some(path) =
                                    PiProvider::session_file(&session_dir, provider_session_id)
                                {
                                    *log_path_lock = Some(path);
                                }
                            }
                        }
                    } else if log_path_lock.is_none() {
                        match snap.provider.as_str() {
                            "codex" => {
                                let agent_home = get_wardian_home()
                                    .map(|home| home.join("agents").join(&snap.session_id))
                                    .filter(|path| path.exists())
                                    .map(|path| path.to_string_lossy().to_string());
                                let codex_session_id =
                                    codex_log_lookup_session_id(snap.resume_session.as_deref())
                                        .map(str::to_string);
                                if let Some(codex_session_id) = codex_session_id {
                                    if let Some(path) = codex_session_file_path(
                                        &codex_session_id,
                                        agent_home.as_deref(),
                                    ) {
                                        *log_path_lock = Some(path);
                                    }
                                }
                            }
                            "claude" => {}
                            _ => {
                                // Gemini: scan ~/.gemini/tmp for chat log files.
                                // Bounded to recent candidates with prefix reads,
                                // and retried only after the backoff TTL when
                                // nothing matched.
                                if let Some(gemini_session_id) = gemini_session_id {
                                    if let Some(home) = dirs::home_dir()
                                        .filter(|_| gemini_fallback_scan_due(&snap.session_id))
                                    {
                                        let tmp_dir = home.join(".gemini").join("tmp");
                                        if let Some(path) =
                                            discover_gemini_log_in_tmp(&tmp_dir, gemini_session_id)
                                        {
                                            *log_path_lock = Some(path);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Provider-aware log parsing for status/query enrichment
                    if let Some(ref path) = *log_path_lock {
                        // Capture admission is independent of the parser's
                        // mtime. A failed archive pass must remain retryable on
                        // an unchanged source during a later ordinary tick.
                        if (wardian_core::identity::normalize_status(&status_before_log_work) == "off"
                            || capture_observation_sessions.contains(&snap.session_id))
                            && path.extension().is_some_and(|extension| extension == "jsonl")
                        {
                            if let Ok(source) = crate::commands::provider_log_acquisition::observe_provider_log_source(path) {
                                background_captures.push(crate::state::background_capture::CaptureRequest {
                                    session_id: snap.session_id.clone(),
                                    incarnation: snap.current_status.clone(),
                                    provider: snap.provider.clone(),
                                    conversation: snap.capture_conversation.clone(),
                                    source_path: Some(path.clone()),
                                    source: Some(source),
                                    logging_enabled: crate::state::conversation_archive::effective_conversation_logging(
                                        global_logging, snap.conversation_logging,
                                    ) == wardian_core::conversations::ConversationLoggingSetting::Enabled,
                                });
                            }
                        }
                        let mut should_parse = true;
                        let mut new_mtime = None;
                        let mut is_initial_log_replay = snap
                            .log_last_modified
                            .lock()
                            .map(|last| last.is_none())
                            .unwrap_or(false);
                        if let Some(modified) =
                            telemetry_source_modified(snap.provider.as_str(), path)
                        {
                            let last_mod = *snap.log_last_modified.lock().unwrap();
                            if last_mod == Some(modified) {
                                should_parse = false;
                            } else {
                                is_initial_log_replay = last_mod.is_none();
                                new_mtime = Some(modified);
                            }
                        }

                        if should_parse {
                            if is_antigravity_database(snap.provider.as_str(), path) {
                                if let Ok(metrics) =
                                    AntigravityProvider::conversation_metrics_from_database(path)
                                {
                                    if let Some(mtime) = new_mtime {
                                        *snap.log_last_modified.lock().unwrap() = Some(mtime);
                                    }
                                    q_count = metrics.query_count;
                                    update_latest_query_timestamp(
                                        &mut last_query_timestamp,
                                        metrics.last_query_timestamp,
                                    );
                                    if let Some(status) = metrics.status {
                                        set_snapshot_status_from_log(
                                            snap,
                                            status,
                                            is_initial_log_replay,
                                        );
                                    }
                                    if metrics.init_timestamp.is_some() {
                                        i_ts = metrics.init_timestamp;
                                    }
                                }
                            } else if let Ok(content) = read_log_bounded(path) {
                                if let Some(mtime) = new_mtime {
                                    *snap.log_last_modified.lock().unwrap() = Some(mtime);
                                }
                                match snap.provider.as_str() {
                                    "codex" => {
                                        let lines: Vec<serde_json::Value> = content
                                            .lines()
                                            .filter_map(|l| serde_json::from_str(l).ok())
                                            .collect();

                                        q_count = lines
                                            .iter()
                                            .filter(|l| {
                                                l.get("type").and_then(|v| v.as_str())
                                                    == Some("event_msg")
                                                    && l.get("payload")
                                                        .and_then(|v| v.get("type"))
                                                        .and_then(|v| v.as_str())
                                                        == Some("user_message")
                                            })
                                            .count();

                                        if let Some(meta) = lines.iter().find(|l| {
                                            l.get("type").and_then(|v| v.as_str())
                                                == Some("session_meta")
                                        }) {
                                            if let Some(ts) = meta
                                                .get("payload")
                                                .and_then(|v| v.get("timestamp"))
                                                .and_then(|v| v.as_str())
                                            {
                                                i_ts = Some(ts.to_string());
                                            }
                                        }

                                        for line in lines.iter().filter(|line| {
                                            line.get("type").and_then(|value| value.as_str())
                                                == Some("event_msg")
                                                && line
                                                    .get("payload")
                                                    .and_then(|value| value.get("type"))
                                                    .and_then(|value| value.as_str())
                                                    == Some("user_message")
                                        }) {
                                            update_latest_query_timestamp(
                                                &mut last_query_timestamp,
                                                query_timestamp_from_value(
                                                    line.get("timestamp").or_else(|| {
                                                        line.get("payload").and_then(|payload| {
                                                            payload.get("timestamp")
                                                        })
                                                    }),
                                                ),
                                            );
                                        }

                                        if let Some(status) = codex_status_from_log(&lines) {
                                            set_snapshot_status_from_log(
                                                snap,
                                                &status,
                                                is_initial_log_replay,
                                            );
                                        }
                                    }
                                    "claude" => {
                                        // Claude logs are JSONL — one JSON object per line
                                        let lines: Vec<serde_json::Value> = content
                                            .lines()
                                            .filter_map(|l| serde_json::from_str(l).ok())
                                            .collect();

                                        q_count = lines
                                            .iter()
                                            .filter(|l| {
                                                l.get("type").and_then(|v| v.as_str())
                                                    == Some("user")
                                                    && claude_is_real_user_query(l)
                                            })
                                            .count();

                                        if let Some(first) = lines.first() {
                                            if let Some(ts) =
                                                first.get("timestamp").and_then(|v| v.as_str())
                                            {
                                                i_ts = Some(ts.to_string());
                                            } else if let Some(ts_num) =
                                                first.get("timestamp").and_then(|v| v.as_i64())
                                            {
                                                // Fallback if timestamp is an epoch number
                                                if let Some(dt) =
                                                    chrono::DateTime::from_timestamp_millis(ts_num)
                                                {
                                                    i_ts = Some(dt.to_rfc3339_opts(
                                                        chrono::SecondsFormat::Millis,
                                                        true,
                                                    ));
                                                }
                                            }
                                        }

                                        for line in lines.iter().filter(|line| {
                                            line.get("type").and_then(|value| value.as_str())
                                                == Some("user")
                                                && claude_is_real_user_query(line)
                                        }) {
                                            update_latest_query_timestamp(
                                                &mut last_query_timestamp,
                                                query_timestamp_from_value(line.get("timestamp")),
                                            );
                                        }

                                        apply_claude_log_status(
                                            snap,
                                            &lines,
                                            is_initial_log_replay,
                                        );
                                    }
                                    "opencode" => {
                                        let mut status = snap.telemetry_status();
                                        let Some(effective_session_id) = opencode_session_id else {
                                            continue;
                                        };
                                        apply_opencode_log_metrics(
                                            &content,
                                            effective_session_id,
                                            &mut q_count,
                                            &mut i_ts,
                                            &mut last_query_timestamp,
                                            &mut status,
                                        );
                                        status = reconcile_live_opencode_log_status(
                                            &snap.provider,
                                            &status_before_log_work,
                                            status,
                                            process_alive,
                                            *snap.last_output_at.lock().unwrap(),
                                        );
                                        if wardian_core::identity::normalize_status(&status)
                                            == "idle"
                                        {
                                            record_latest_opencode_assistant_text(
                                                snap,
                                                effective_session_id,
                                            );
                                        }
                                        set_snapshot_status_from_log(
                                            snap,
                                            &status,
                                            is_initial_log_replay,
                                        );
                                    }
                                    "antigravity" => {
                                        let (queries, start_time, status, latest_query) =
                                            parse_antigravity_log_metrics(&content);
                                        q_count = queries;
                                        update_latest_query_timestamp(
                                            &mut last_query_timestamp,
                                            latest_query,
                                        );
                                        if let Some(status) = status {
                                            set_snapshot_status_from_log(
                                                snap,
                                                status,
                                                is_initial_log_replay,
                                            );
                                        }
                                        if start_time.is_some() {
                                            i_ts = start_time;
                                        }
                                        record_latest_antigravity_assistant_text(snap, &content);
                                    }
                                    "pi" => {
                                        if let Some(metrics) = parse_pi_log_metrics(&content) {
                                            q_count = metrics.query_count;
                                            if let Some(start_time) = metrics.init_timestamp {
                                                i_ts = Some(start_time);
                                            }
                                            update_latest_query_timestamp(
                                                &mut last_query_timestamp,
                                                metrics.last_query_timestamp,
                                            );
                                        }
                                    }
                                    _ => {
                                        if let Some(metrics) = parse_gemini_log_metrics(&content) {
                                            q_count = metrics.query_count;
                                            if let Some(status) = metrics.status {
                                                set_snapshot_status_from_log(
                                                    snap,
                                                    status,
                                                    is_initial_log_replay,
                                                );
                                            }
                                            if let Some(start_time) = metrics.init_timestamp {
                                                i_ts = Some(start_time);
                                            }
                                            update_latest_query_timestamp(
                                                &mut last_query_timestamp,
                                                metrics.last_query_timestamp,
                                            );
                                        }
                                        if snap.provider == "gemini" {
                                            record_latest_gemini_assistant_text(snap, &content);
                                        }
                                    }
                                }
                                if is_initial_log_replay {
                                    update_latest_query_timestamp(
                                        &mut last_query_timestamp,
                                        latest_query_timestamp_from_log_suffix(
                                            path,
                                            snap.provider.as_str(),
                                        ),
                                    );
                                }
                            }
                        }
                    }

                    if q_count > 0 {
                        *snap.query_count.lock().unwrap() = q_count;
                    }
                    if let Some(ts) = i_ts {
                        *snap.init_timestamp.lock().unwrap() = Some(ts);
                    }
                    log_path_display = log_path_lock.as_ref().map(|p| display_log_path(p));
                } else {
                    crate::utils::logging::log_debug(&format!(
                        "[Wardian] Skipped overlapping telemetry log work for {}",
                        snap.session_id
                    ));
                }
            }

            reconcile_cached_last_query_timestamp(
                &mut last_query_timestamp,
                &snap.last_query_timestamp,
            );
            if let Some(timestamp) = last_query_timestamp.as_deref() {
                let _db_clock = PhaseClock::start(&db_total);
                let _ = wardian_core::db::update_agent_query_timestamp(&snap.session_id, timestamp);
            }

            if (snap.provider == "opencode"
                || snap.provider == "claude"
                || snap.provider == "antigravity")
                && (snap.process_id.is_none() || process_alive == Some(true))
            {
                let current_status = snap.telemetry_status();
                let last_output_at = *snap.last_output_at.lock().unwrap();
                if provider_should_fallback_to_idle_after_quiet_period(
                    &current_status,
                    last_output_at,
                    std::time::SystemTime::now(),
                ) {
                    set_snapshot_status(snap, "Idle");
                }
            }

            // If the process has terminated, force status to "Off" so the UI
            // doesn't stay stuck on "Processing..." or "Action Needed".
            if process_alive == Some(false) && snap.process_id.is_some() {
                set_snapshot_status(snap, "Off");
            }

            let observed_status = snap.telemetry_status();
            let is_offline = snap.is_off
                || matches!(
                    wardian_core::identity::normalize_status(&observed_status).as_str(),
                    "off" | "error"
                );
            let active_execution_conflict =
                wardian_core::conversation_lease::find_active_execution_conflict(
                    &active_leases,
                    &snap.session_id,
                    snap.resume_session.as_deref().unwrap_or_default(),
                    &lease_now,
                )
                .is_some();
            let current_status = if is_offline && active_execution_conflict {
                "Headless".to_string()
            } else {
                observed_status
            };
            provider_statuses.push(snap.provider_status_observation(active_execution_conflict));

            results.push(AgentTelemetry {
                session_id: snap.session_id.clone(),
                cpu_usage: cpu,
                memory_mb: mem,
                uptime_seconds: uptime,
                query_count: *snap.query_count.lock().unwrap(),
                init_timestamp: snap.init_timestamp.lock().unwrap().clone(),
                last_query_timestamp,
                current_status,
                log_path: log_path_display,
            });
            let agent_duration = agent_started.elapsed();
            if agent_duration >= TELEMETRY_SLOW_PASS_THRESHOLD {
                slow_agents.push(TelemetrySlowAgent {
                    session_id: snap.session_id.clone(),
                    provider: snap.provider.clone(),
                    duration: agent_duration,
                });
            }
        }
        let sys_refresh = system_snapshot
            .as_ref()
            .map(|snapshot| snapshot.sys_refresh)
            .unwrap_or_default();
        let timings = TelemetryPassTimings {
            total: pass_started.elapsed(),
            sys_refresh,
            setup: loop_started
                .duration_since(pass_started)
                .saturating_sub(sys_refresh),
            agents: loop_started.elapsed(),
            log: log_total.get(),
            db: db_total.get(),
            agent_count: snapshots.len(),
            slow_agents,
        };
        if let Some(message) = timings.slow_log_message(TELEMETRY_SLOW_PASS_THRESHOLD) {
            crate::utils::logging::log_debug(&message);
        }
        TelemetryPassResult {
            metrics: results,
            provider_statuses,
            background_captures,
        }
    })
    .await
    .unwrap_or_default();
    let mut result = result;
    let publish_started = std::time::Instant::now();
    let mut follow_up =
        apply_provider_status_observations(state, &result.provider_statuses, &mut result.metrics)
            .await;
    follow_up.background_captures = result.background_captures;
    let publish = publish_started.elapsed();
    if pre_pass.max(publish) >= TELEMETRY_SLOW_PASS_THRESHOLD {
        crate::utils::logging::log_debug(&format!(
            "[Wardian] Slow status publication pre_pass_ms={} publish_ms={} agent_count={}",
            pre_pass.as_millis(),
            publish.as_millis(),
            result.metrics.len()
        ));
    }
    (result.metrics, follow_up)
}
