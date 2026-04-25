//! Rule: cancel-unsafe-in-select
//!
//! Detects non-cancel-safe futures used directly inside `tokio::select!`
//! arms.
//!
//! ## Why
//!
//! `tokio::select!` polls multiple futures concurrently and only one wins.
//! The losing futures are dropped mid-poll. If a losing future has internal
//! state that gets discarded on drop — like buffered bytes from
//! `read_exact` — the next call to that I/O primitive starts in the wrong
//! place. Bytes are silently lost.
//!
//! The classic failure mode is a length-prefixed framed protocol:
//! `read_exact` partially consumes a length prefix, gets dropped, and the
//! next read interprets the second half of the prefix (or the start of
//! the payload) as a fresh length. The wire desyncs — symptoms include
//! `frame too large: 1818192238 bytes` errors where the "length" is
//! actually four ASCII characters from a streaming payload.
//!
//! ## Fix
//!
//! Move the cancel-unsafe call into a dedicated tokio task that owns the
//! reader and forwards parsed messages over an `mpsc` channel. The
//! `select!` arm then awaits `channel.recv()`, which IS cancel-safe.
//!
//! ## Scope
//!
//! Only the future-expression position is flagged (the LHS of `=>`).
//! Calls inside the arm's handler block are fine — once an arm wins, its
//! handler runs to completion without being dropped.

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range};
use tree_sitter::{Node, Parser};

/// Tokio I/O primitives that are not cancel-safe.
///
/// Each loses internal progress (buffered bytes, partial writes) when
/// dropped mid-future. See the `tokio::select!` docs and each method's
/// "Cancel safety" note.
const CANCEL_UNSAFE: &[&str] = &[
    // tokio::io::AsyncReadExt
    "read_exact",
    "read_to_end",
    "read_to_string",
    "read_buf",
    // tokio::io::AsyncBufReadExt
    "read_line",
    "read_until",
    // tokio::io::AsyncWriteExt
    "write_all",
    "write_buf",
    "write_all_buf",
];

/// Entry point: parse `source` and return all diagnostics using the
/// built-in cancel-unsafe list only.
///
/// For project-specific wrappers (e.g. a function that delegates to
/// `read_exact` internally), use `check_cancel_unsafe_in_select_with`
/// and pass the wrapper names as `extra`.
pub fn check_cancel_unsafe_in_select(source: &str) -> Vec<Diagnostic> {
    let empty: &[&str] = &[];
    check_cancel_unsafe_in_select_with(source, empty)
}

/// Like `check_cancel_unsafe_in_select`, but also flags calls to any
/// name in `extra`. Use this to flag project-local wrappers that
/// transitively call cancel-unsafe primitives — the rule itself can't
/// follow function bodies across files.
pub fn check_cancel_unsafe_in_select_with<S: AsRef<str>>(
    source: &str,
    extra: &[S],
) -> Vec<Diagnostic> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::language())
        .expect("failed to load tree-sitter-rust grammar");

    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => return vec![],
    };

    let source_bytes = source.as_bytes();
    let mut diagnostics = Vec::new();
    let extra_refs: Vec<&str> = extra.iter().map(|s| s.as_ref()).collect();
    walk(
        tree.root_node(),
        source_bytes,
        &extra_refs,
        &mut diagnostics,
    );
    diagnostics
}

/// Recursively walk the tree, analyzing every `select!` macro invocation.
fn walk(node: Node, source: &[u8], extra: &[&str], diagnostics: &mut Vec<Diagnostic>) {
    if node.kind() == "macro_invocation" && is_select_macro(node, source) {
        analyze_select(node, source, extra, diagnostics);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, source, extra, diagnostics);
    }
}

/// Returns true if the macro path's last segment is `select` — matches
/// `select!`, `tokio::select!`, `tokio::select_biased!`-style names are
/// excluded because they end in `_biased` not `select`.
fn is_select_macro(node: Node, source: &[u8]) -> bool {
    let macro_path = match node.child_by_field_name("macro") {
        Some(n) => n,
        None => return false,
    };
    let path_text = node_text(macro_path, source);
    path_text.rsplit("::").next().unwrap_or("") == "select"
}

/// Extract arm-future text ranges from a `select!` body and emit a
/// diagnostic for each cancel-unsafe call inside one.
fn analyze_select(
    macro_node: Node,
    source: &[u8],
    extra: &[&str],
    diagnostics: &mut Vec<Diagnostic>,
) {
    // The macro_invocation text includes the path and the body. We slice
    // from the first `{` to the matching `}` so the depth tracker starts
    // inside the body, not outside it.
    let macro_text = node_text(macro_node, source);
    let body_start_in_macro = match macro_text.find('{') {
        Some(i) => i + 1,
        None => return,
    };
    let body_end_in_macro = match macro_text.rfind('}') {
        Some(i) => i,
        None => return,
    };
    if body_end_in_macro <= body_start_in_macro {
        return;
    }
    let body = &macro_text[body_start_in_macro..body_end_in_macro];
    let body_offset = macro_node.start_byte() + body_start_in_macro;

    for (start, end) in extract_arm_future_ranges(body) {
        let arm = &body[start..end];
        let mut emit = |name: &str| {
            for offset in find_call_positions(arm, name) {
                let abs_start = body_offset + start + offset;
                let abs_end = abs_start + name.len();
                diagnostics.push(make_diagnostic(source, abs_start, abs_end, name));
            }
        };
        for &name in CANCEL_UNSAFE {
            emit(name);
        }
        for &name in extra {
            // Skip names already in the built-in list to avoid duplicate
            // diagnostics if a user lists e.g. "read_exact" in their config.
            if !CANCEL_UNSAFE.contains(&name) {
                emit(name);
            }
        }
    }
}

/// Walk `body` and return `(start, end)` byte offsets for each
/// arm-future expression — the text between `<pat> =` and the matching
/// `=>`, at brace/paren depth 0.
///
/// The scanner tracks string/char literals and `//` / `/* */` comments
/// so `=` and `=>` inside them don't confuse arm boundaries.
fn extract_arm_future_ranges(body: &str) -> Vec<(usize, usize)> {
    let bytes = body.as_bytes();
    let mut ranges = Vec::new();
    let mut depth: i32 = 0;
    let mut i = 0usize;
    let mut future_start: Option<usize> = None;

    while i < bytes.len() {
        let b = bytes[i];

        // Line comment
        if b == b'/' && bytes.get(i + 1).copied() == Some(b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // Block comment (does not handle nesting; that's a Rust extension
        // we can add later if it shows up in fixtures).
        if b == b'/' && bytes.get(i + 1).copied() == Some(b'*') {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = i.saturating_add(2);
            continue;
        }

        // String literal
        if b == b'"' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            i = i.saturating_add(1);
            continue;
        }

        // Char literal vs lifetime. A char literal closes within a small
        // window: `'a'`, `'\n'`, `'\u{...}'`. A lifetime is `'<ident>`
        // with no closing quote on the same identifier. We probe for a
        // closing quote within a short distance; if found, treat as char
        // and skip; otherwise advance one byte (lifetime).
        if b == b'\'' {
            let close = (1..=12)
                .filter_map(|n| bytes.get(i + n).copied().map(|c| (n, c)))
                .find(|&(_, c)| c == b'\'')
                .map(|(n, _)| n);
            if let Some(end_off) = close {
                i += end_off + 1;
                continue;
            }
            // lifetime: just advance
            i += 1;
            continue;
        }

        match b {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth -= 1,
            b'=' if depth == 0 => {
                let next = bytes.get(i + 1).copied();
                let prev = if i > 0 { Some(bytes[i - 1]) } else { None };

                if next == Some(b'>') {
                    // `=>` closes the current arm-future.
                    if let Some(start) = future_start.take() {
                        ranges.push((start, i));
                    }
                    i += 2;
                    continue;
                }

                let is_compound = next == Some(b'=')
                    || prev == Some(b'<')
                    || prev == Some(b'>')
                    || prev == Some(b'!')
                    || prev == Some(b'=');

                if !is_compound && future_start.is_none() {
                    future_start = Some(i + 1);
                }
            }
            b',' | b';' if depth == 0 => {
                // End of arm body — reset arm-future tracking. If we're
                // still inside a future expression here, it means the arm
                // had no `=>` (malformed); discard it.
                future_start = None;
            }
            _ => {}
        }

        i += 1;
    }

    ranges
}

/// Return byte offsets within `text` where `name` appears as the head of
/// a function call: `name(` with optional whitespace, surrounded by word
/// boundaries.
fn find_call_positions(text: &str, name: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let name_bytes = name.as_bytes();
    let mut positions = Vec::new();

    if name_bytes.is_empty() || name_bytes.len() > bytes.len() {
        return positions;
    }

    let mut i = 0usize;
    while i + name_bytes.len() <= bytes.len() {
        if &bytes[i..i + name_bytes.len()] == name_bytes {
            let prev_is_word = i > 0 && is_word_char(bytes[i - 1]);
            let after_name = i + name_bytes.len();
            let next_is_word = after_name < bytes.len() && is_word_char(bytes[after_name]);
            if !prev_is_word && !next_is_word {
                let mut j = after_name;
                while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'(' {
                    positions.push(i);
                    i += name_bytes.len();
                    continue;
                }
            }
        }
        i += 1;
    }
    positions
}

fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn make_diagnostic(source: &[u8], start_byte: usize, end_byte: usize, name: &str) -> Diagnostic {
    Diagnostic {
        range: byte_range_to_lsp_range(source, start_byte, end_byte),
        severity: Some(DiagnosticSeverity::WARNING),
        code: Some(NumberOrString::String(
            "async-rust/cancel-unsafe-in-select".to_string(),
        )),
        code_description: None,
        source: Some("async-rust-lsp".to_string()),
        message: format!(
            "`{}` is not cancel-safe. Inside a `tokio::select!` arm, this future may be \
             dropped mid-poll if a sibling arm wins the race — discarding any bytes already \
             consumed (or written). The wire silently desyncs. Move this call into a \
             dedicated reader/writer task that forwards parsed messages over an `mpsc` \
             channel; then `select!` on `channel.recv()`, which is cancel-safe.",
            name
        ),
        related_information: None,
        tags: None,
        data: None,
    }
}

fn node_text<'a>(node: Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

fn byte_range_to_lsp_range(source: &[u8], start: usize, end: usize) -> Range {
    Range {
        start: byte_to_position(source, start),
        end: byte_to_position(source, end),
    }
}

fn byte_to_position(source: &[u8], byte: usize) -> Position {
    let mut line: u32 = 0;
    let mut col: u32 = 0;
    let limit = byte.min(source.len());
    for &b in &source[..limit] {
        if b == b'\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    Position {
        line,
        character: col,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn diag_count(source: &str) -> usize {
        check_cancel_unsafe_in_select(source).len()
    }

    fn diag_messages(source: &str) -> Vec<String> {
        check_cancel_unsafe_in_select(source)
            .into_iter()
            .map(|d| d.message)
            .collect()
    }

    // --- BAD patterns ---

    #[test]
    fn detects_read_exact_in_select_arm_future() {
        let src = r#"
async fn bad(reader: &mut R) {
    let mut buf = [0u8; 4];
    tokio::select! {
        n = reader.read_exact(&mut buf) => println!("read {:?}", n),
        _ = sleep_for_a_bit() => println!("timeout"),
    }
}
"#;
        assert_eq!(
            diag_count(src),
            1,
            "expected 1 diagnostic for read_exact in select arm, got: {:?}",
            diag_messages(src)
        );
    }

    #[test]
    fn detects_qualified_path_call_in_select_arm() {
        // The classic nteract failure mode: `connection::recv_typed_frame`.
        // The function name `recv_typed_frame` isn't in our default list,
        // but its body calls `read_exact`. We detect at the leaf: anyone
        // who writes `read_exact` directly in a select arm is flagged.
        let src = r#"
async fn bad(reader: &mut R) {
    tokio::select! {
        frame = read_full_frame(reader) => Some(frame),
        cmd = cmd_rx.recv() => None,
    };
}

async fn read_full_frame<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) -> Vec<u8> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await.unwrap();
    Vec::new()
}
"#;
        // The select! itself doesn't contain `read_exact` directly, so
        // zero diagnostics in the macro. The `read_exact` call lives
        // inside the helper function, outside any select! — also no
        // diagnostic. This is by design; we flag only direct uses.
        assert_eq!(
            diag_count(src),
            0,
            "should only flag direct unsafe calls inside select! arms"
        );
    }

    #[test]
    fn detects_write_all_in_select_arm() {
        let src = r#"
async fn bad(writer: &mut W, payload: &[u8]) {
    tokio::select! {
        _ = writer.write_all(payload) => (),
        _ = sleep_for_a_bit() => (),
    }
}
"#;
        assert_eq!(diag_count(src), 1);
    }

    #[test]
    fn detects_read_to_end_in_tokio_select() {
        let src = r#"
async fn bad(reader: &mut R) {
    let mut buf = Vec::new();
    tokio::select! {
        _ = reader.read_to_end(&mut buf) => (),
        _ = something_else() => (),
    }
}
"#;
        assert_eq!(diag_count(src), 1);
    }

    #[test]
    fn detects_multiple_unsafe_calls_in_one_select() {
        let src = r#"
async fn bad(reader: &mut R, writer: &mut W) {
    let mut buf = [0u8; 4];
    tokio::select! {
        _ = reader.read_exact(&mut buf) => (),
        _ = writer.write_all(&buf) => (),
    }
}
"#;
        assert_eq!(
            diag_count(src),
            2,
            "should flag both arms: {:?}",
            diag_messages(src)
        );
    }

    #[test]
    fn detects_with_biased_keyword() {
        let src = r#"
async fn bad(reader: &mut R) {
    let mut buf = [0u8; 4];
    tokio::select! {
        biased;
        n = reader.read_exact(&mut buf) => Some(n),
        _ = sleep_for_a_bit() => None,
    };
}
"#;
        assert_eq!(diag_count(src), 1);
    }

    #[test]
    fn detects_in_unqualified_select() {
        // Some codebases `use tokio::select` and write `select! { ... }`
        let src = r#"
async fn bad(reader: &mut R) {
    let mut buf = [0u8; 4];
    select! {
        n = reader.read_exact(&mut buf) => n,
        _ = sleep_for_a_bit() => Default::default(),
    };
}
"#;
        assert_eq!(diag_count(src), 1);
    }

    #[test]
    fn diagnostic_code_is_correct() {
        let src = r#"
async fn bad(reader: &mut R) {
    let mut buf = [0u8; 4];
    tokio::select! {
        _ = reader.read_exact(&mut buf) => (),
        _ = something_else() => (),
    }
}
"#;
        let diags = check_cancel_unsafe_in_select(src);
        assert_eq!(diags.len(), 1);
        assert_eq!(
            diags[0].code,
            Some(NumberOrString::String(
                "async-rust/cancel-unsafe-in-select".to_string()
            ))
        );
    }

    // --- GOOD patterns ---

    #[test]
    fn no_diagnostic_for_cancel_safe_recv() {
        let src = r#"
async fn good(rx: &mut tokio::sync::mpsc::Receiver<u8>) {
    tokio::select! {
        msg = rx.recv() => println!("{:?}", msg),
        _ = sleep_for_a_bit() => (),
    }
}
"#;
        assert_eq!(diag_count(src), 0);
    }

    #[test]
    fn no_diagnostic_for_call_inside_arm_handler_block() {
        // A `read_exact` call inside the handler block is fine — by the
        // time the block runs, the arm has already won.
        let src = r#"
async fn good(reader: &mut R) {
    let mut buf = [0u8; 4];
    tokio::select! {
        msg = some_safe_future() => {
            reader.read_exact(&mut buf).await.unwrap();
        }
        _ = sleep_for_a_bit() => (),
    }
}
"#;
        assert_eq!(
            diag_count(src),
            0,
            "should not flag read_exact in handler block: {:?}",
            diag_messages(src)
        );
    }

    #[test]
    fn no_diagnostic_outside_select_macro() {
        let src = r#"
async fn fine(reader: &mut R) {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf).await.unwrap();
}
"#;
        assert_eq!(diag_count(src), 0);
    }

    #[test]
    fn no_diagnostic_for_method_named_recv_on_channel() {
        let src = r#"
async fn good(rx: &mut Receiver, framed: &mut FramedReader) {
    tokio::select! {
        msg = rx.recv() => Some(msg),
        frame = framed.recv() => None,
    };
}
"#;
        assert_eq!(diag_count(src), 0);
    }

    #[test]
    fn no_diagnostic_for_string_literal_with_unsafe_name() {
        // A string literal containing the name "read_exact" should not
        // be flagged.
        let src = r#"
async fn good() {
    tokio::select! {
        msg = log("read_exact called") => println!("{}", msg),
        _ = sleep_for_a_bit() => (),
    }
}
"#;
        assert_eq!(diag_count(src), 0);
    }

    #[test]
    fn no_diagnostic_for_comment_in_select() {
        let src = r#"
async fn good() {
    tokio::select! {
        // We avoid read_exact here because it's not cancel-safe
        msg = safe_recv() => Some(msg),
        _ = sleep_for_a_bit() => None,
    };
}
"#;
        assert_eq!(diag_count(src), 0);
    }

    #[test]
    fn empty_source_produces_no_diagnostics() {
        assert_eq!(diag_count(""), 0);
    }

    #[test]
    fn no_diagnostic_for_select_biased_macro_other_name() {
        // A macro literally named `select_biased!` (different crate) is
        // out of scope — only `select` matches.
        let src = r#"
async fn outside_scope(reader: &mut R) {
    let mut buf = [0u8; 4];
    select_biased! {
        n = reader.read_exact(&mut buf) => Some(n),
        _ = sleep_for_a_bit() => None,
    }
}
"#;
        assert_eq!(diag_count(src), 0);
    }

    #[test]
    fn diagnostic_position_is_at_method_name() {
        let src = r#"
async fn bad(reader: &mut R) {
    let mut buf = [0u8; 4];
    tokio::select! {
        n = reader.read_exact(&mut buf) => Some(n),
        _ = sleep_for_a_bit() => None,
    };
}
"#;
        let diags = check_cancel_unsafe_in_select(src);
        assert_eq!(diags.len(), 1);
        // The diagnostic should be on the line with `read_exact`
        let line = diags[0].range.start.line;
        let lines: Vec<&str> = src.lines().collect();
        assert!(
            lines[line as usize].contains("read_exact"),
            "diagnostic line {} should contain read_exact, got: {:?}",
            line,
            lines[line as usize]
        );
    }

    // --- Internal scanner tests ---

    #[test]
    fn extract_arm_future_ranges_simple() {
        let body = " a = future_a() => x, b = future_b() => y, ";
        let ranges = extract_arm_future_ranges(body);
        assert_eq!(ranges.len(), 2);
        let f0 = &body[ranges[0].0..ranges[0].1];
        let f1 = &body[ranges[1].0..ranges[1].1];
        assert!(f0.contains("future_a()"), "got {:?}", f0);
        assert!(f1.contains("future_b()"), "got {:?}", f1);
    }

    #[test]
    fn extract_arm_future_ranges_with_biased() {
        let body = " biased; a = future_a() => x, ";
        let ranges = extract_arm_future_ranges(body);
        assert_eq!(ranges.len(), 1);
        let f = &body[ranges[0].0..ranges[0].1];
        assert!(f.contains("future_a()"), "got {:?}", f);
    }

    #[test]
    fn extract_arm_future_ranges_skips_arm_body() {
        // Calls inside `=> { ... }` should not appear in any returned
        // range.
        let body = " a = future_a() => { reader.read_exact(buf); }, ";
        let ranges = extract_arm_future_ranges(body);
        assert_eq!(ranges.len(), 1);
        let f = &body[ranges[0].0..ranges[0].1];
        assert!(!f.contains("read_exact"), "got {:?}", f);
        assert!(f.contains("future_a()"), "got {:?}", f);
    }

    #[test]
    fn find_call_positions_basic() {
        let p = find_call_positions("foo.read_exact(buf)", "read_exact");
        assert_eq!(p, vec![4]);
    }

    #[test]
    fn find_call_positions_word_boundary() {
        // `read_exact_extra` should NOT match `read_exact`
        let p = find_call_positions("foo.read_exact_extra(buf)", "read_exact");
        assert!(p.is_empty(), "got {:?}", p);
    }

    #[test]
    fn find_call_positions_must_be_followed_by_paren() {
        let p = find_call_positions("foo.read_exact = 5", "read_exact");
        assert!(p.is_empty(), "got {:?}", p);
    }

    // --- Project-level extras ---

    #[test]
    fn extras_flag_project_wrappers() {
        // The exact pre-fix nteract pattern: `recv_typed_frame` is a
        // wrapper around `read_exact`. The default rule misses it; the
        // extras list catches it.
        let src = r#"
async fn the_bug_we_fixed<R>(reader: &mut R, rx: &mut Receiver<u8>) {
    tokio::select! {
        biased;
        frame = connection::recv_typed_frame(&mut reader) => println!("{:?}", frame),
        cmd = rx.recv() => println!("{:?}", cmd),
    };
}
"#;
        // Default list misses the wrapper.
        assert_eq!(check_cancel_unsafe_in_select(src).len(), 0);

        // With the wrapper in extras, we catch it.
        let extras = ["recv_typed_frame", "send_typed_frame"];
        let diags = check_cancel_unsafe_in_select_with(src, &extras);
        assert_eq!(diags.len(), 1, "got: {:?}", diags);
        assert!(
            diags[0].message.contains("recv_typed_frame"),
            "diagnostic should name the wrapper, got: {}",
            diags[0].message
        );
    }

    #[test]
    fn extras_does_not_double_flag_built_in_names() {
        // Listing `read_exact` (already built-in) in extras must not
        // produce two diagnostics.
        let src = r#"
async fn bad(reader: &mut R) {
    let mut buf = [0u8; 4];
    tokio::select! {
        _ = reader.read_exact(&mut buf) => (),
        _ = sleep_for_a_bit() => (),
    }
}
"#;
        let extras = ["read_exact"];
        let diags = check_cancel_unsafe_in_select_with(src, &extras);
        assert_eq!(diags.len(), 1, "got: {:?}", diags);
    }

    #[test]
    fn extras_accepts_string_slice() {
        // The `_with` variant should accept owned Strings too, not just
        // &str literals — this is the LSP-config plumbing path.
        let src = r#"
async fn bad(reader: &mut R) {
    tokio::select! {
        _ = my_wrapper(reader) => (),
        _ = sleep_for_a_bit() => (),
    }
}
"#;
        let extras: Vec<String> = vec!["my_wrapper".to_string()];
        let diags = check_cancel_unsafe_in_select_with(src, &extras);
        assert_eq!(diags.len(), 1, "got: {:?}", diags);
    }
}
