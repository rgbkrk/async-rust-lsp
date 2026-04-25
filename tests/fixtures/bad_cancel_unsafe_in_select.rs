// FIXTURE: bad patterns — each async fn below should produce at least one
// diagnostic from the `cancel-unsafe-in-select` rule.

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

/// Classic framed-protocol failure mode. `read_exact` partial reads are
/// dropped when the other arm wins; the next read interprets payload
/// bytes as a fresh length prefix. The symptom in production logs is a
/// nonsense length like `frame too large: 1818192238 bytes` — four
/// bytes of streaming payload reinterpreted as a header.
async fn read_exact_in_select<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    rx: &mut mpsc::Receiver<u8>,
) {
    let mut len_buf = [0u8; 4];
    tokio::select! {
        biased;
        _ = reader.read_exact(&mut len_buf) => println!("read len"),
        _ = rx.recv() => println!("got cmd"),
    }
}

/// Same failure mode with `read_to_end`.
async fn read_to_end_in_select<R: AsyncReadExt + Unpin>(reader: &mut R) {
    let mut buf = Vec::new();
    tokio::select! {
        _ = reader.read_to_end(&mut buf) => (),
        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => (),
    }
}

/// `read_to_string` has the same problem.
async fn read_to_string_in_select<R: AsyncReadExt + Unpin>(reader: &mut R) {
    let mut s = String::new();
    tokio::select! {
        _ = reader.read_to_string(&mut s) => (),
        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => (),
    }
}

/// `read_buf` drops bytes when cancelled.
async fn read_buf_in_select<R: AsyncReadExt + Unpin>(reader: &mut R) {
    let mut buf = bytes::BytesMut::with_capacity(64);
    tokio::select! {
        _ = reader.read_buf(&mut buf) => (),
        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => (),
    }
}

/// `write_all` leaves partial writes when cancelled — the receiver sees
/// a half-written frame and desyncs.
async fn write_all_in_select<W: AsyncWriteExt + Unpin>(writer: &mut W, payload: &[u8]) {
    tokio::select! {
        _ = writer.write_all(payload) => (),
        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => (),
    }
}

/// `read_line` from `AsyncBufReadExt` drops buffered bytes on cancel.
async fn read_line_in_select<R: AsyncBufReadExt + Unpin>(reader: &mut R) {
    let mut line = String::new();
    tokio::select! {
        _ = reader.read_line(&mut line) => (),
        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => (),
    }
}

/// `read_until` is the byte-oriented variant of `read_line` and has the
/// same cancel-safety problem.
async fn read_until_in_select<R: AsyncBufReadExt + Unpin>(reader: &mut R) {
    let mut buf = Vec::new();
    tokio::select! {
        _ = reader.read_until(b'\n', &mut buf) => (),
        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => (),
    }
}

/// Two cancel-unsafe arms in one select! — both should be flagged.
async fn read_and_write_both_unsafe<R: AsyncReadExt + Unpin, W: AsyncWriteExt + Unpin>(
    reader: &mut R,
    writer: &mut W,
    payload: &[u8],
) {
    let mut buf = [0u8; 4];
    tokio::select! {
        _ = reader.read_exact(&mut buf) => (),
        _ = writer.write_all(payload) => (),
    }
}

/// Bare `select!` (without the `tokio::` prefix). Catch this even when
/// the user does `use tokio::select;` at the top of the file.
async fn unqualified_select<R: AsyncReadExt + Unpin>(reader: &mut R) {
    let mut buf = [0u8; 4];
    select! {
        _ = reader.read_exact(&mut buf) => (),
        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => (),
    }
}
