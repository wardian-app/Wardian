use super::*;
use crate::control::test_support::TestWardianHome;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;

async fn bound(state: &InteractionState) -> (u64, store::TaskClaim) {
    let generation = state
        .start_provider_input_generation("receiver", ProviderInputReadiness::Ready, None)
        .await
        .generation;
    state
        .admit_agent_message(store::Admission {
            sender: "requester",
            recipient: "receiver",
            message: "literal λ中 task",
            idempotency_key: None,
            task: true,
            generation,
        })
        .await
        .unwrap();
    let claim = store::with_db(|conn| store::claim_next_task(conn, "receiver", generation))
        .unwrap()
        .unwrap();
    state
        .bind_agent_task_turn(&claim, "thread", "A", "steer")
        .await
        .unwrap();
    (generation, claim)
}

#[tokio::test]
async fn reply_while_lookup_waits_excludes_the_canonical_task() {
    let _home = TestWardianHome::new_async().await;
    let state = InteractionState::default();
    let (generation, claim) = bound(&state).await;
    let guard = state.mutation_lock.lock().await;
    let mut read = Box::pin(state.read_bound_task_contexts(
        "receiver",
        generation,
        "thread",
        "A",
        || Ok(()),
        Ok,
    ));
    std::future::poll_fn(|cx| {
        assert!(read.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    // The legitimate settlement writer holds the same mutation boundary. The
    // cached record remains old, so this also detects an in-memory fallback.
    store::with_db(|conn| {
        store::reply(
            conn,
            "receiver",
            &claim.record.id,
            ReplyStatus::Done,
            "explicit result",
        )
    })
    .unwrap();
    drop(guard);
    assert_eq!(read.await.unwrap_err().code, "task_context_unavailable");
}

#[tokio::test]
async fn serialization_keeps_settlement_excluded_and_rechecks_native_publication() {
    let _home = TestWardianHome::new_async().await;
    let state = InteractionState::default();
    let (generation, _claim) = bound(&state).await;
    let native_current = AtomicBool::new(true);
    let result = state
        .read_bound_task_contexts(
            "receiver",
            generation,
            "thread",
            "A",
            || {
                if native_current.load(Ordering::Acquire) {
                    Ok(())
                } else {
                    Err(AgentMessagingError::new(
                        "stale_task_context",
                        "Native turn changed",
                    ))
                }
            },
            |tasks| {
                assert_eq!(tasks[0].message.body, "literal λ中 task");
                assert!(state.mutation_lock.try_lock().is_err());
                native_current.store(false, Ordering::Release);
                Ok(tasks)
            },
        )
        .await;
    assert_eq!(result.unwrap_err().code, "stale_task_context");
    assert!(state.mutation_lock.try_lock().is_ok());
    let replaced = state
        .start_provider_input_generation("receiver", ProviderInputReadiness::Ready, None)
        .await
        .generation;
    assert_ne!(generation, replaced);
    assert_eq!(
        state
            .read_bound_task_contexts("receiver", generation, "thread", "A", || Ok(()), Ok)
            .await
            .unwrap_err()
            .code,
        "stale_task_context"
    );
}
