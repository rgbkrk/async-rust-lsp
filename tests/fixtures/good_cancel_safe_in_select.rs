// FIXTURE: good patterns — none of these should produce diagnostics from
// the `cancel-unsafe-in-select` rule.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

/// Cancel-safe channel receive — `mpsc::Receiver::recv` is documented as
/// cancel-safe. This is the recommended fix when the original code used
/// `read_exact` directly.
async fn cancel_safe_recv(rx: &mut mpsc::Receiver<u8>) {
    tokio::select! {
        msg = rx.recv() => println!("{:?}", msg),
        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => (),
    }
}

/// `read_exact` inside the arm's *handler* block is fine — by the time
/// the block executes, this arm has already won the race and won't be
/// dropped.
async fn unsafe_call_in_handler_block_is_ok<R: AsyncReadExt + Unpin>(reader: &mut R) {
    let mut buf = [0u8; 4];
    tokio::select! {
        msg = some_safe_future() => {
            // This `read_exact` runs to completion; the future was the
            // cancel-safe `some_safe_future()`, not this call.
            reader.read_exact(&mut buf).await.unwrap();
            println!("{:?} {:?}", msg, buf);
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => (),
    }
}

/// `framed_reader.recv()` is cancel-safe because `FramedReader` is an
/// actor: the `read_exact` call lives inside a dedicated task, and the
/// `select!` arm only awaits an `mpsc::Receiver`. The canonical fix for
/// a cancel-unsafe framed reader is this actor boundary.
async fn actor_pattern_is_safe(framed: &mut FramedReader) {
    tokio::select! {
        frame = framed.recv() => println!("{:?}", frame),
        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => (),
    }
}

/// `read_exact` outside any `select!` is not flagged — cancel safety
/// only matters when the future can be dropped mid-poll.
async fn outside_select_is_fine<R: AsyncReadExt + Unpin>(reader: &mut R) {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf).await.unwrap();
}

/// String literal containing the name of a cancel-unsafe primitive must
/// not trigger a false positive.
async fn name_in_string_literal() {
    tokio::select! {
        msg = log("read_exact called") => println!("{}", msg),
        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => (),
    }
}

/// Comment containing the name must not trigger a false positive.
async fn name_in_comment(rx: &mut mpsc::Receiver<u8>) {
    tokio::select! {
        // Important: do NOT use read_exact here, it's not cancel-safe.
        msg = rx.recv() => println!("{:?}", msg),
        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => (),
    }
}

/// A user-defined method that happens to share a *prefix* with a known
/// cancel-unsafe name (`read_exact_extra`) must not be flagged.
async fn lookalike_method_is_not_flagged(reader: &mut MyReader) {
    let mut buf = [0u8; 4];
    tokio::select! {
        _ = reader.read_exact_extra(&mut buf) => (),
        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => (),
    }
}

/// Simple helpers and stubs so the fixture parses cleanly.
async fn some_safe_future() -> u8 {
    0
}

async fn log(_msg: &str) -> &'static str {
    "ok"
}

struct FramedReader;

impl FramedReader {
    async fn recv(&mut self) -> Option<Vec<u8>> {
        None
    }
}

struct MyReader;

impl MyReader {
    async fn read_exact_extra(&mut self, _buf: &mut [u8]) {}
}
