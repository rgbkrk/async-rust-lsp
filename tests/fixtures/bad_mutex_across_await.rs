// FIXTURE: bad patterns — each async fn below should produce at least one diagnostic.

use tokio::sync::{Mutex, RwLock};
use std::sync::Arc;

/// Classic case: tokio Mutex guard held across a plain `.await`.
/// This is exactly the nteract desktop deadlock pattern.
async fn basic_mutex_across_await() {
    let mutex = Mutex::new(0u32);
    let guard = mutex.lock().await;   // acquires guard
    do_work(*guard);
    tokio::time::sleep(std::time::Duration::from_millis(1)).await; // ← DEADLOCK RISK
    // guard drops here (end of fn) but it was held across the await above
}

/// RwLock write guard held across an await.
async fn rwlock_write_across_await() {
    let lock = RwLock::new(Vec::<u8>::new());
    let mut guard = lock.write().await;
    guard.push(1);
    some_async_op().await; // ← DEADLOCK RISK — write lock still held
}

/// RwLock read guard held across an await.
async fn rwlock_read_across_await() {
    let lock = RwLock::new(0u32);
    let guard = lock.read().await;
    println!("{}", *guard);
    some_async_op().await; // ← DEADLOCK RISK — read lock still held
}

/// Guard through a field access — common in real code (state machine pattern).
async fn field_access_mutex_across_await(state: Arc<AppState>) {
    let guard = state.counter.lock().await;
    process(&guard);
    notify_clients().await; // ← DEADLOCK RISK — guard on state.counter still held
}

/// Multiple guards from the same block — each await after either guard is flagged.
async fn multiple_guards_across_await() {
    let m1 = Mutex::new(1u32);
    let m2 = Mutex::new(2u32);
    let g1 = m1.lock().await;
    let g2 = m2.lock().await;
    do_work(*g1 + *g2);
    flush().await; // ← DEADLOCK RISK — both guards live here
}

// Stub types/functions for the fixture to parse cleanly
struct AppState {
    counter: Mutex<u32>,
}

fn do_work(_: u32) {}
fn process(_: &u32) {}
async fn some_async_op() {}
async fn notify_clients() {}
async fn flush() {}
