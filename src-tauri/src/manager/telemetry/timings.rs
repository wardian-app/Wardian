//! Timing for one status-sampling pass, and the slow-pass log line built from it.

use std::cell::Cell;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub(super) struct TelemetrySlowAgent {
    pub(super) session_id: String,
    pub(super) provider: String,
    pub(super) duration: Duration,
}

#[derive(Debug, Clone, Default)]
pub(super) struct TelemetryPassTimings {
    pub(super) total: Duration,
    pub(super) sys_refresh: Duration,
    pub(super) agent_count: usize,
    pub(super) slow_agents: Vec<TelemetrySlowAgent>,
    /// Everything before the per-agent loop that is not the process refresh:
    /// lease load, the user-message timestamp query, Codex index observation.
    pub(super) setup: Duration,
    /// The per-agent loop.
    pub(super) agents: Duration,
    /// Provider log discovery and parsing, summed across agents.
    pub(super) log: Duration,
    /// Durable query-timestamp writes, summed across agents.
    pub(super) db: Duration,
}

impl TelemetryPassTimings {
    pub(super) fn slow_log_message(&self, threshold: Duration) -> Option<String> {
        if self.total < threshold && self.sys_refresh < threshold && self.slow_agents.is_empty() {
            return None;
        }

        let slow_agents = if self.slow_agents.is_empty() {
            "none".to_string()
        } else {
            self.slow_agents
                .iter()
                .map(|agent| {
                    format!(
                        "{}:{}:{}ms",
                        agent.session_id,
                        agent.provider,
                        agent.duration.as_millis()
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
        };

        Some(format!(
            "[Wardian] Slow telemetry pass total_ms={} sys_refresh_ms={} setup_ms={} agents_ms={} log_ms={} db_ms={} agent_count={} slow_agents={}",
            self.total.as_millis(),
            self.sys_refresh.as_millis(),
            self.setup.as_millis(),
            self.agents.as_millis(),
            self.log.as_millis(),
            self.db.as_millis(),
            self.agent_count,
            slow_agents
        ))
    }
}

/// Adds the time between its creation and its drop to a running total, so a
/// phase is counted even when the code inside it leaves early with `continue`.
pub(super) struct PhaseClock<'a> {
    total: &'a Cell<Duration>,
    started: Instant,
}

impl<'a> PhaseClock<'a> {
    pub(super) fn start(total: &'a Cell<Duration>) -> Self {
        Self {
            total,
            started: Instant::now(),
        }
    }
}

impl Drop for PhaseClock<'_> {
    fn drop(&mut self) {
        self.total.set(self.total.get() + self.started.elapsed());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slow_pass_line_attributes_the_time_to_its_phases() {
        let report = TelemetryPassTimings {
            total: Duration::from_millis(1300),
            sys_refresh: Duration::from_millis(40),
            setup: Duration::from_millis(60),
            agents: Duration::from_millis(1200),
            log: Duration::from_millis(900),
            db: Duration::from_millis(210),
            agent_count: 66,
            ..Default::default()
        };

        let message = report
            .slow_log_message(Duration::from_millis(500))
            .expect("a 1.3 s pass is slow");

        for expected in [
            "total_ms=1300",
            "sys_refresh_ms=40",
            "setup_ms=60",
            "agents_ms=1200",
            "log_ms=900",
            "db_ms=210",
            "agent_count=66",
        ] {
            assert!(message.contains(expected), "{expected} missing: {message}");
        }
    }

    #[test]
    fn a_phase_clock_counts_time_even_when_the_phase_leaves_early() {
        let total = Cell::new(Duration::ZERO);

        for index in 0..2 {
            let _clock = PhaseClock::start(&total);
            std::thread::sleep(Duration::from_millis(20));
            if index == 0 {
                continue;
            }
        }

        assert!(total.get() >= Duration::from_millis(40));
    }
}
