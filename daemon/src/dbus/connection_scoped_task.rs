// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! The epoch-guarded disconnect-cleanup protocol shared by `SetOutputSuppressed`
//! (ticket 24) and `StartDepthStream` / `StopDepthStream` (ticket 26), carved
//! into one place (post-release ticket 16).
//!
//! Both features run a **connection-scoped background task**: one that must be
//! **superseded** when the same method is called again — from this connection
//! or another — and **torn down** when the connection that armed it drops
//! without an explicit stop. Before this module the mechanism was spelled out
//! six times across two hand-rolled `{ epoch, watcher }` structs and four
//! methods. The rule is:
//!
//! > bump the epoch and abort the old watcher **in one locked section**; store
//! > the new handle (or run the disconnect-clear) **only if the epoch still
//! > matches** the value captured in that section.
//!
//! zbus spawns a fresh task per incoming method call (every method takes
//! `&self`, never `&mut self`), so two calls can genuinely run concurrently.
//! The epoch bump and the old watcher's take/abort therefore have to happen in
//! one atomic critical section, and the epoch compared at store time must be
//! *this* call's own bumped value — not whatever the shared `epoch` happens to
//! read moments later after an `.await` — or a racing call's bump could get
//! misattributed and this call's store could clobber a newer call's
//! already-stored watcher.
//!
//! [`ConnectionScopedTask::supersede`] does the locked bump+abort and hands
//! back an [`ArmToken`] carrying that captured epoch. [`ArmToken::finish`]
//! spawns the task and stores its handle iff the epoch is still current;
//! dropping the token unfinished leaves the task disarmed, which is exactly
//! what [`ConnectionScopedTask::disarm`] does. `finish` performs no `.await`
//! before the spawn, so the supersede→store window is inherently atomic: a
//! concurrent full `supersede`+`finish` lands entirely before this token
//! (stale, handle aborted) or entirely after (this handle stored, then
//! superseded and aborted) — never interleaved.

use std::future::Future;
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use tokio::task::JoinHandle;

/// A background task tied to the lifetime of one D-Bus connection: superseded
/// when the same method re-arms it, torn down when the arming connection
/// drops. Owns the epoch guard, the `JoinHandle`, and `watch_disconnect`.
///
/// `Clone` is an `Arc` bump — `Daemon` holds one per protocol by value and the
/// spawned task closes over a clone. The `Mutex`, the `u64`, and the
/// `JoinHandle` are module-private; no caller ever names them.
#[derive(Clone)]
pub(super) struct ConnectionScopedTask {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    epoch: u64,
    watcher: Option<JoinHandle<()>>,
}

/// The still-current-at-store proof minted by [`ConnectionScopedTask::supersede`].
/// Valid only until the next `supersede`; dropping it unfinished leaves the
/// task disarmed. `'static` and holds no lock, so it is safe to carry across
/// an `.await` (the injector call in `set_output_suppressed`).
pub(super) struct ArmToken {
    inner: Arc<Mutex<Inner>>,
    epoch: u64,
}

impl ConnectionScopedTask {
    pub(super) fn new() -> Self {
        ConnectionScopedTask {
            inner: Arc::new(Mutex::new(Inner {
                epoch: 0,
                watcher: None,
            })),
        }
    }

    /// Bump the epoch and abort the current watcher in one locked section, and
    /// capture the bumped epoch into the returned [`ArmToken`].
    #[must_use]
    pub(super) fn supersede(&self) -> ArmToken {
        let mut inner = self.inner.lock().unwrap();
        inner.epoch += 1;
        if let Some(handle) = inner.watcher.take() {
            handle.abort();
        }
        let epoch = inner.epoch;
        drop(inner);
        ArmToken {
            inner: self.inner.clone(),
            epoch,
        }
    }

    /// `supersede` then drop — bump + abort, no re-arm.
    pub(super) fn disarm(&self) {
        let _ = self.supersede();
    }
}

impl ArmToken {
    /// Spawn the scoped task and store its handle — but only if no `supersede`
    /// has run since this token was minted (checked against the epoch captured
    /// at `supersede` time, exactly as the store-if-current check does across
    /// its `.await`). The spawned task races `watch_disconnect(connection,
    /// sender)` against `pump`; whichever resolves first ends it, and then —
    /// iff this arming is still current — the watcher slot is cleared and
    /// `on_clear` is awaited.
    ///
    /// No `.await` before `tokio::spawn`: the supersede (in `supersede`) and
    /// the store (below) are both lock-only, so with nothing awaited between
    /// them a concurrent full supersede+finish never interleaves.
    pub(super) fn finish<F>(
        self,
        connection: zbus::Connection,
        sender: Option<String>,
        pump: impl Future<Output = ()> + Send + 'static,
        on_clear: impl FnOnce() -> F + Send + 'static,
    ) where
        F: Future<Output = ()> + Send + 'static,
    {
        let ArmToken { inner, epoch } = self;
        let task_inner = inner.clone();
        let handle = tokio::spawn(async move {
            tokio::select! {
                _ = watch_disconnect(connection, sender) => {}
                _ = pump => {}
            }

            // Only clear the shared state if this task's own epoch is still
            // current — a since-superseded task's late cleanup must never
            // clobber a newer call's already-stored watcher.
            let still_current = {
                let mut guard = task_inner.lock().unwrap();
                if guard.epoch == epoch {
                    guard.watcher = None;
                    true
                } else {
                    false
                }
            };
            if still_current {
                on_clear().await;
            }
        });

        // Only claim the watcher slot if no concurrent call has superseded this
        // one since the token was minted — otherwise abort immediately rather
        // than overwrite whatever that newer call already stored.
        let mut guard = inner.lock().unwrap();
        if guard.epoch == epoch {
            guard.watcher = Some(handle);
        } else {
            handle.abort();
        }
    }
}

/// Waits for whichever connection armed the scoped task — a
/// `SetOutputSuppressed(true)` or a `StartDepthStream` — to disconnect. On a
/// real message bus (`connection.is_bus()`), that's a
/// specific client among potentially several sharing the Daemon's one
/// session-bus connection, so it's tracked by unique name via
/// `org.freedesktop.DBus`'s `NameOwnerChanged` (the standard idiom for
/// "notice when my caller goes away" on a shared bus — zbus's own
/// `Connection::close_when_bus_name_disappears`-style tests use the same
/// `(0, name), (2, "")` match-arg filter). Over a private peer-to-peer
/// connection (the test harness's `TestServer`, and any future non-bus
/// transport), there is no bus daemon and no unique name to watch, but the
/// `zbus::Connection` itself *is* the one peer, so its own close detection
/// is exact.
async fn watch_disconnect(connection: zbus::Connection, sender: Option<String>) {
    if connection.is_bus()
        && let Some(sender) = sender
        && let Ok(dbus) = zbus::fdo::DBusProxy::new(&connection).await
        && let Ok(mut stream) = dbus
            .receive_name_owner_changed_with_args(&[(0, sender.as_str()), (2, "")])
            .await
    {
        // Subscribed *before* checking current ownership, never the other
        // way around, so a disconnect racing this setup can never be
        // missed: if the name is already gone by the time we check, either
        // it vanished before the subscription above took effect (caught by
        // this check) or after (already queued in `stream`, caught by
        // `stream.next()` below either way).
        if let Ok(name) = zbus::names::BusName::try_from(sender.as_str())
            && let Ok(false) = dbus.name_has_owner(name).await
        {
            return;
        }
        stream.next().await;
        return;
    }
    connection.closed().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// A bare peer-to-peer `zbus::Connection` pair — no object server, no
    /// dispatch, no injector, no config file. Dropping `client` closes the
    /// server side's socket, which is all `watch_disconnect`'s non-bus path
    /// needs.
    async fn p2p_pair() -> (zbus::Connection, zbus::Connection) {
        let (server_transport, client_transport) = tokio::net::UnixStream::pair().unwrap();
        let guid = zbus::Guid::generate();
        let server_builder = zbus::connection::Builder::unix_stream(server_transport)
            .server(guid)
            .unwrap()
            .p2p();
        let client_builder = zbus::connection::Builder::unix_stream(client_transport).p2p();
        let (server, client) = tokio::join!(server_builder.build(), client_builder.build());
        (server.unwrap(), client.unwrap())
    }

    /// Flips `flag` when dropped — i.e. when the task holding it is aborted.
    struct AbortSentinel(Arc<AtomicBool>);

    impl Drop for AbortSentinel {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// A `pump` that never completes on its own. Flips `started` on its first
    /// poll and `dropped` if the task carrying it is later aborted (an async
    /// block that is dropped before its first poll runs no destructors, so a
    /// pump has to be polled once before the abort sentinel means anything).
    async fn tracked_pump(started: Arc<AtomicBool>, dropped: Arc<AtomicBool>) {
        started.store(true, Ordering::SeqCst);
        let _sentinel = AbortSentinel(dropped);
        std::future::pending::<()>().await
    }

    async fn eventually(mut cond: impl FnMut() -> bool) {
        for _ in 0..200 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("condition was never met");
    }

    #[tokio::test]
    async fn supersede_aborts_the_prior_task() {
        let (server, client) = p2p_pair().await;
        let task = ConnectionScopedTask::new();

        let first_started = Arc::new(AtomicBool::new(false));
        let first_dropped = Arc::new(AtomicBool::new(false));
        task.supersede().finish(
            server.clone(),
            None,
            tracked_pump(first_started.clone(), first_dropped.clone()),
            || async {},
        );
        eventually(|| first_started.load(Ordering::SeqCst)).await;

        let second_started = Arc::new(AtomicBool::new(false));
        let second_dropped = Arc::new(AtomicBool::new(false));
        task.supersede().finish(
            server.clone(),
            None,
            tracked_pump(second_started.clone(), second_dropped.clone()),
            || async {},
        );

        eventually(|| first_dropped.load(Ordering::SeqCst)).await;
        eventually(|| second_started.load(Ordering::SeqCst)).await;
        assert!(
            !second_dropped.load(Ordering::SeqCst),
            "the re-arming task must still be running"
        );

        drop(client);
    }

    #[tokio::test]
    async fn a_stale_token_does_not_store_its_handle() {
        let (server, client) = p2p_pair().await;
        let task = ConnectionScopedTask::new();

        let stale = task.supersede();
        let current = task.supersede();

        let stale_started = Arc::new(AtomicBool::new(false));
        stale.finish(
            server.clone(),
            None,
            tracked_pump(stale_started.clone(), Arc::new(AtomicBool::new(false))),
            || async {},
        );
        assert!(
            task.inner.lock().unwrap().watcher.is_none(),
            "a stale token must not claim the watcher slot"
        );

        let current_started = Arc::new(AtomicBool::new(false));
        current.finish(
            server.clone(),
            None,
            tracked_pump(current_started.clone(), Arc::new(AtomicBool::new(false))),
            || async {},
        );
        assert!(
            task.inner.lock().unwrap().watcher.is_some(),
            "the current token stores its handle"
        );

        eventually(|| current_started.load(Ordering::SeqCst)).await;
        assert!(
            !stale_started.load(Ordering::SeqCst),
            "the stale token's task must have been aborted before it ever ran"
        );

        drop(client);
    }

    #[tokio::test]
    async fn a_disconnect_runs_on_clear_when_the_arming_is_still_current() {
        let (server, client) = p2p_pair().await;
        let task = ConnectionScopedTask::new();

        let cleared = Arc::new(AtomicBool::new(false));
        let cleared_in_task = cleared.clone();
        task.supersede().finish(
            server.clone(),
            None,
            std::future::pending(),
            move || async move {
                cleared_in_task.store(true, Ordering::SeqCst);
            },
        );

        drop(client);

        eventually(|| cleared.load(Ordering::SeqCst)).await;
        assert!(
            task.inner.lock().unwrap().watcher.is_none(),
            "teardown clears the watcher slot"
        );
    }

    #[tokio::test]
    async fn a_disconnect_on_a_superseded_arming_never_runs_its_on_clear() {
        let (server_a, client_a) = p2p_pair().await;
        let task = ConnectionScopedTask::new();

        let cleared_a = Arc::new(AtomicBool::new(false));
        let cleared_a_in_task = cleared_a.clone();
        task.supersede().finish(
            server_a.clone(),
            None,
            std::future::pending(),
            move || async move {
                cleared_a_in_task.store(true, Ordering::SeqCst);
            },
        );

        // A newer arming supersedes A.
        let (server_b, client_b) = p2p_pair().await;
        task.supersede()
            .finish(server_b.clone(), None, std::future::pending(), || async {});

        // A's connection dropping must now clear nothing — the epoch has moved.
        drop(client_a);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !cleared_a.load(Ordering::SeqCst),
            "a since-superseded arming's disconnect must not run its on_clear"
        );
        assert!(
            task.inner.lock().unwrap().watcher.is_some(),
            "the current (B) arming still holds the slot"
        );

        drop(client_b);
    }

    #[tokio::test]
    async fn a_pending_pump_still_ends_the_task_on_disconnect() {
        let (server, client) = p2p_pair().await;
        let task = ConnectionScopedTask::new();

        task.supersede()
            .finish(server.clone(), None, std::future::pending(), || async {});
        assert!(task.inner.lock().unwrap().watcher.is_some());

        drop(client);
        eventually(|| task.inner.lock().unwrap().watcher.is_none()).await;
    }
}
