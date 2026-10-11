//! Real loopback protocol tests with a virtual clock, without a provider process.
use super::*;

struct Fixture {
    plan: PiBridgeLaunchPlan,
    port: u16,
    _directory: tempfile::TempDir,
    clock_guard: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.clock_guard.abort();
    }
}

impl Fixture {
    async fn new() -> Self {
        // Tokio otherwise advances paused time while real Windows socket I/O
        // is pending. Keep a runnable task so only explicit advance() calls
        // can expire the protocol's timers.
        let clock_guard = tokio::spawn(async {
            loop {
                tokio::task::yield_now().await;
            }
        });
        let directory = tempfile::tempdir().unwrap();
        let extension = directory.path().join("extension.mjs");
        std::fs::write(&extension, "// test extension").unwrap();
        let plan = PiBridgeLaunchPlan::prepare(
            "agent-startup".into(),
            7,
            "session-startup".into(),
            directory.path().join("session.jsonl"),
            extension,
        )
        .await
        .unwrap();
        let config: Value = serde_json::from_str(plan.config()).unwrap();
        Self {
            port: config["port"].as_u64().unwrap() as u16,
            plan,
            _directory: directory,
            clock_guard,
        }
    }

    async fn connect(&self) -> TcpStream {
        TcpStream::connect(("127.0.0.1", self.port)).await.unwrap()
    }

    fn state(&self) -> PiBridgeStartup {
        self.plan.owner.startup.borrow().clone()
    }

    async fn hello(&self, stream: &mut TcpStream) {
        let mut hello = common_frame(&self.plan.owner, "runtime-startup", 1, "hello");
        hello.insert("token".into(), self.plan.owner.token.clone().into());
        hello.insert("pid".into(), 42.into());
        hello.insert(
            "session_file".into(),
            self.plan.owner.binding.session_file.clone().into(),
        );
        write_frame(stream, &hello).await.unwrap();
        assert_eq!(read_frame(stream).await.unwrap()["type"], "welcome");
    }

    async fn ready(&self, stream: &mut TcpStream) {
        let mut ready = common_frame(&self.plan.owner, "runtime-startup", 2, "ready");
        ready.insert(
            "capabilities".into(),
            serde_json::json!({"task": true, "information": false, "cancel": false, "completion": false}),
        );
        write_frame(stream, &ready).await.unwrap();
        // Socket writes do not imply the listener has published readiness.
        // Wait for that observable state with a real-time bound, leaving the
        // virtual protocol clock unchanged.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while self.state() == PiBridgeStartup::Pending {
            assert!(
                std::time::Instant::now() < deadline,
                "readiness was not published"
            );
            tokio::task::yield_now().await;
        }
        assert_eq!(self.state(), PiBridgeStartup::Ready);
        assert!(self.plan.owner.is_ready());
    }
}

// Yield without waiting on a timer, so paused time advances only where requested.
async fn pump() {
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
}

async fn advance(seconds: u64) {
    tokio::time::advance(Duration::from_secs(seconds)).await;
    pump().await;
}

fn assert_failed(fixture: &Fixture, reason: &str) {
    let PiBridgeStartup::Failed(actual) = fixture.state() else {
        panic!("expected startup failure, got {:?}", fixture.state());
    };
    assert!(actual.contains(reason), "{actual}");
    assert!(!fixture.plan.owner.is_ready());
}

#[tokio::test(start_paused = true)]
async fn preparation_has_no_timer_until_actual_process_registration() {
    let fixture = Fixture::new().await;
    pump().await;
    fixture.plan.register_process(0);
    advance(40).await;
    assert_eq!(fixture.state(), PiBridgeStartup::Pending);
    fixture.plan.register_process(42);
    let mut stream = fixture.connect().await;
    fixture.hello(&mut stream).await;
    fixture.ready(&mut stream).await;
}

#[tokio::test(start_paused = true)]
async fn valid_connection_after_six_seconds_can_become_ready() {
    let fixture = Fixture::new().await;
    fixture.plan.register_process(42);
    pump().await;
    advance(6).await;
    assert_eq!(fixture.state(), PiBridgeStartup::Pending);
    let mut stream = fixture.connect().await;
    fixture.hello(&mut stream).await;
    fixture.ready(&mut stream).await;
}

#[tokio::test(start_paused = true)]
async fn early_connection_waits_for_actual_process_registration() {
    let fixture = Fixture::new().await;
    let mut stream = fixture.connect().await;
    advance(6).await;
    assert_eq!(fixture.state(), PiBridgeStartup::Pending);
    fixture.plan.register_process(42);
    fixture.hello(&mut stream).await;
    fixture.ready(&mut stream).await;
}

#[tokio::test(start_paused = true)]
async fn stalled_hello_retains_its_five_second_limit() {
    let fixture = Fixture::new().await;
    fixture.plan.register_process(42);
    let _stream = fixture.connect().await;
    pump().await;
    advance(4).await;
    assert_eq!(fixture.state(), PiBridgeStartup::Pending);
    advance(1).await;
    assert_failed(&fixture, "hello/timeout");
}

#[tokio::test(start_paused = true)]
async fn stalled_ready_retains_its_five_second_limit() {
    let fixture = Fixture::new().await;
    fixture.plan.register_process(42);
    let mut stream = fixture.connect().await;
    fixture.hello(&mut stream).await;
    advance(4).await;
    assert_eq!(fixture.state(), PiBridgeStartup::Pending);
    advance(1).await;
    assert_failed(&fixture, "ready/timeout");
}

#[tokio::test(start_paused = true)]
async fn authentication_cannot_extend_the_overall_startup_budget() {
    let fixture = Fixture::new().await;
    fixture.plan.register_process(42);
    pump().await;
    advance(34).await;
    let mut stream = fixture.connect().await;
    fixture.hello(&mut stream).await;
    advance(1).await;
    assert!(matches!(fixture.state(), PiBridgeStartup::Failed(_)));
    assert!(!fixture.plan.owner.is_ready());
}

#[tokio::test(start_paused = true)]
async fn duplicate_registration_cannot_replace_identity_or_reset_deadline() {
    let fixture = Fixture::new().await;
    fixture.plan.register_process(42);
    pump().await;
    advance(30).await;
    fixture.plan.register_process(99);
    let mut stream = fixture.connect().await;
    fixture.hello(&mut stream).await;
    advance(5).await;
    assert!(matches!(fixture.state(), PiBridgeStartup::Failed(_)));
}

#[tokio::test(start_paused = true)]
async fn invalid_hello_is_rejected_without_waiting_for_startup_deadline() {
    let fixture = Fixture::new().await;
    fixture.plan.register_process(42);
    let mut stream = fixture.connect().await;
    write_frame(&mut stream, &Map::new()).await.unwrap();
    pump().await;
    assert_failed(&fixture, "hello/invalid_shape");
}

#[tokio::test(start_paused = true)]
async fn stale_generation_never_becomes_ready() {
    let fixture = Fixture::new().await;
    fixture.plan.register_process(42);
    let mut stream = fixture.connect().await;
    let mut hello = common_frame(&fixture.plan.owner, "runtime-startup", 1, "hello");
    hello.insert("generation".into(), 6.into());
    hello.insert("token".into(), fixture.plan.owner.token.clone().into());
    hello.insert("pid".into(), 42.into());
    hello.insert(
        "session_file".into(),
        fixture.plan.owner.binding.session_file.clone().into(),
    );
    write_frame(&mut stream, &hello).await.unwrap();
    pump().await;
    assert_failed(&fixture, "hello/binding_mismatch");
}

#[tokio::test(start_paused = true)]
async fn close_cancels_every_startup_phase_without_revival() {
    for phase in 0..4 {
        let fixture = Fixture::new().await;
        let mut stream = None;
        if phase > 0 {
            fixture.plan.register_process(42);
        }
        if phase > 1 {
            stream = Some(fixture.connect().await);
        }
        if phase > 2 {
            fixture.hello(stream.as_mut().unwrap()).await;
        }
        pump().await;
        fixture.plan.owner.close();
        fixture.plan.register_process(99);
        pump().await;
        assert_failed(&fixture, "closed before authenticated readiness");
        assert!(TcpStream::connect(("127.0.0.1", fixture.port))
            .await
            .is_err());
    }
}

#[tokio::test(start_paused = true)]
async fn dropping_unattached_plan_disposes_unregistered_listener() {
    let fixture = Fixture::new().await;
    let port = fixture.port;
    let owner = fixture.plan.owner();
    drop(fixture);
    pump().await;
    assert!(matches!(
        *owner.startup.borrow(),
        PiBridgeStartup::Failed(_)
    ));
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
}
