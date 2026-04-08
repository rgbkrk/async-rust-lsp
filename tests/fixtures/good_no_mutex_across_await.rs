// FIXTURE: good patterns — none of these should produce diagnostics.

use tokio::sync::{Mutex, RwLock};
use std::sync::Arc;

/// Guard is scoped to a nested block; it's dropped before the .await.
async fn scoped_guard_before_await() {
    let mutex = Mutex::new(0u32);
    let value = {
        let guard = mutex.lock().await;
        *guard // copies value out, guard drops at end of block
    };
    tokio::time::sleep(std::time::Duration::from_millis(1)).await; // fine — no guard held
    println!("{}", value);
}

/// Explicit drop() before the await.
async fn explicit_drop_before_await() {
    let mutex = Mutex::new(0u32);
    let guard = mutex.lock().await;
    let value = *guard;
    drop(guard); // explicitly released
    some_async_op().await; // fine
    println!("{}", value);
}

/// Await only comes from the lock acquisition itself; no subsequent await.
async fn lock_then_no_await() {
    let mutex = Mutex::new(0u32);
    let guard = mutex.lock().await;
    let _value = *guard;
    // fn returns here, guard drops, no subsequent .await
}

/// std::sync::Mutex is synchronous — guard is not from an await_expression.
/// Clippy handles this; we don't re-flag it.
async fn std_mutex_is_not_flagged() {
    let mutex = std::sync::Mutex::new(0u32);
    let guard = mutex.lock().unwrap(); // sync, not an await_expression
    let _value = *guard;
    drop(guard);
    some_async_op().await; // fine — std guard already dropped
}

/// RwLock read guard scoped out before await.
async fn rwlock_read_scoped() {
    let lock = RwLock::new(vec![1u8, 2, 3]);
    let snapshot: Vec<u8> = {
        let guard = lock.read().await;
        guard.clone()
    };
    some_async_op().await; // fine — guard dropped in block above
    println!("{:?}", snapshot);
}

/// RwLock write guard with explicit drop.
async fn rwlock_write_explicit_drop() {
    let lock = RwLock::new(0u32);
    let mut guard = lock.write().await;
    *guard = 42;
    drop(guard);
    some_async_op().await; // fine
}

/// No async at all — should not produce diagnostics.
fn sync_function() {
    let x = 42u32;
    println!("{}", x);
}

// Stub
async fn some_async_op() {}
