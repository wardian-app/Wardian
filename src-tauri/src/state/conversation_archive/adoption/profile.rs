//! Opt-in synthetic-test timing. This module never exists in a product build.
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) struct Mark {
    pub phase: &'static str,
    pub at: Duration,
    pub cursor: u64,
}

struct Run {
    agent: String,
    start: Instant,
    marks: Vec<Mark>,
}

static RUN: Mutex<Option<Run>> = Mutex::new(None);

pub(crate) struct Profile;

impl Profile {
    pub fn start(agent: &str) -> Self {
        let mut run = RUN.lock().unwrap();
        assert!(
            run.is_none(),
            "only one synthetic profile may own the collector"
        );
        *run = Some(Run {
            agent: agent.into(),
            start: Instant::now(),
            marks: Vec::new(),
        });
        Self
    }

    pub fn finish(self) -> Vec<Mark> {
        let run = RUN.lock().unwrap().take().unwrap();
        run.marks
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        *RUN.lock().unwrap() = None;
    }
}

pub(crate) fn mark(agent: &str, phase: &'static str, cursor: u64) {
    if let Some(run) = RUN
        .lock()
        .unwrap()
        .as_mut()
        .filter(|run| run.agent == agent)
    {
        run.marks.push(Mark {
            phase,
            at: run.start.elapsed(),
            cursor,
        });
        eprintln!(
            "SYNTHETIC_ADOPTION_PHASE {} {phase} {cursor}",
            run.start.elapsed().as_micros()
        );
        if phase == "adoption_checkpoint" {
            eprintln!(
                "SYNTHETIC_ADOPTION_CHECKPOINT {} {cursor}",
                run.start.elapsed().as_micros()
            );
        }
    }
}
