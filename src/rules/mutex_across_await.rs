//! Rule: tokio-mutex-across-await
//!
//! Detects patterns where a `tokio::sync::Mutex` or `tokio::sync::RwLock` guard
//! is bound in a `let` statement and then a `.await` expression appears later in
//! the same block *before* the guard is explicitly dropped.
//!
//! ## Why tree-sitter?
//!
//! We use tree-sitter for incremental, fault-tolerant parsing. This handles
//! partially-written code (common in editors) gracefully.
//!
//! ## Detection strategy
//!
//! Pattern (conservative, catches the nteract-style deadlock):
//!
//! ```text
//! // BAD — guard lives across the await
//! let guard = some_mutex.lock().await;
//! do_something(&guard);
//! some_future.await; // ← deadlock risk
//!
//! // OK — guard is dropped before the await
//! let value = {
//!     let guard = some_mutex.lock().await;
//!     guard.clone()
//! }; // guard dropped here
//! some_future.await; // fine
//! ```
//!
//! We walk every `block` node. Within a block, we track:
//! 1. `let` bindings whose RHS ends with `.lock().await`, `.write().await`, or
//!    `.read().await` (tokio async lock acquisition).
//! 2. Any subsequent `await_expression` in the same block.
//! 3. Any explicit `drop(guard_name)` call or end-of-scope before an await.
//!
//! If a guard binding is still live when we see a subsequent `.await`, we emit
//! a `Warning` diagnostic at the `.await` site.

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range};
use tree_sitter::{Node, Parser};

/// Entry point: parse `source` and return all diagnostics.
pub fn check_mutex_across_await(source: &str) -> Vec<Diagnostic> {
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

    // Walk all nodes looking for block expressions
    walk_blocks(tree.root_node(), source_bytes, &mut diagnostics);

    diagnostics
}

/// Recursively walk the syntax tree, analyzing each `block` node.
fn walk_blocks(node: Node, source: &[u8], diagnostics: &mut Vec<Diagnostic>) {
    if node.kind() == "block" {
        analyze_block(node, source, diagnostics);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_blocks(child, source, diagnostics);
    }
}

/// A guard binding discovered in a block.
#[derive(Debug, Clone)]
struct GuardBinding {
    /// Name bound by the `let` statement (e.g. `guard`, `lock`)
    name: String,
    /// Byte offset of the `let` statement start (for range ordering)
    start_byte: usize,
}

/// Analyze a single `block` node for the mutex-across-await antipattern.
fn analyze_block(block: Node, source: &[u8], diagnostics: &mut Vec<Diagnostic>) {
    // Collect statements in order
    let mut guards: Vec<GuardBinding> = Vec::new();

    let mut cursor = block.walk();
    for stmt in block.children(&mut cursor) {
        match stmt.kind() {
            // `let <pat> = <expr>;`
            "let_declaration" => {
                // Check the RHS for awaits against already-live guards.
                // This catches e.g. `let id = other.write().await` while a
                // prior guard is still live.
                if let Some(value) = stmt.child_by_field_name("value") {
                    check_await_in_expr(value, source, &guards, diagnostics);
                }

                // If this let shadows an existing guard name, the old guard
                // is implicitly dropped (Rust ownership move/drop semantics).
                if let Some(pattern) = stmt.child_by_field_name("pattern") {
                    let name = node_text(pattern, source);
                    guards.retain(|g| g.name != name);
                }

                // Then register this binding as a new guard if applicable.
                // Order matters: scan first so the new binding doesn't flag
                // against its own `.await`.
                if let Some(binding) = extract_guard_binding(stmt, source) {
                    guards.push(binding);
                }
            }

            // `<expr>;` or bare `<expr>` (last expr in block)
            "expression_statement" | "expression" => {
                // Check for explicit drop() calls to remove live guards
                if let Some(dropped) = extract_drop_call(stmt, source) {
                    guards.retain(|g| g.name != dropped);
                }

                // Check if this expression contains an await
                check_await_in_expr(stmt, source, &guards, diagnostics);
            }

            // Scoped block — guards introduced *before* this nested block that survive
            // into it are still in scope. We don't recurse here for the enclosing
            // block's guards; walk_blocks handles recursion into the nested block
            // with its own fresh guard list.
            _ => {}
        }
    }
}

/// Returns `Some(GuardBinding)` if `node` is a `let` whose initializer is an
/// async lock acquisition: `<expr>.lock().await`, `.write().await`, `.read().await`.
fn extract_guard_binding(let_node: Node, source: &[u8]) -> Option<GuardBinding> {
    // Pattern: `let <pattern> = <value_expression>;`
    // The value is the last named child before the optional `;`

    // Get the bound name from the pattern
    let pattern = let_node.child_by_field_name("pattern")?;
    let name = node_text(pattern, source).to_string();

    // Get the initializer expression
    let value = let_node.child_by_field_name("value")?;

    // Check if the value is an await expression whose inner call is a lock method
    if is_async_lock_call(value, source) {
        Some(GuardBinding {
            name,
            start_byte: let_node.start_byte(),
        })
    } else {
        None
    }
}

/// Returns true if `node` represents `<expr>.lock().await`,
/// `<expr>.write().await`, or `<expr>.read().await`.
fn is_async_lock_call(node: Node, source: &[u8]) -> bool {
    // We expect: await_expression → call_expression → field_expression
    //
    // Tree structure for `mutex.lock().await`:
    //   await_expression
    //     call_expression          (the inner `mutex.lock()`)
    //       field_expression       (`mutex.lock`)
    //         <receiver>           (`mutex`)
    //         field_name: "lock"
    //     "await"
    //
    // But also handle: `self.mutex.lock().await`, `self.state.lock().await`, etc.

    if node.kind() != "await_expression" {
        return false;
    }

    // The expression being awaited
    let inner = match node.named_child(0) {
        Some(n) => n,
        None => return false,
    };

    is_lock_call(inner, source)
}

/// Returns true if `node` is `<expr>.lock()`, `.write()`, or `.read()`.
fn is_lock_call(node: Node, source: &[u8]) -> bool {
    if node.kind() != "call_expression" {
        return false;
    }

    let function = match node.child_by_field_name("function") {
        Some(f) => f,
        None => return false,
    };

    if function.kind() != "field_expression" {
        return false;
    }

    let field = match function.child_by_field_name("field") {
        Some(f) => f,
        None => return false,
    };

    let method_name = node_text(field, source);
    matches!(method_name, "lock" | "write" | "read")
}

/// If `node` is a statement containing `drop(<ident>)`, return the dropped name.
fn extract_drop_call(stmt: Node, source: &[u8]) -> Option<String> {
    // Look for a call_expression with function = "drop" and one arg
    let expr = first_expr_in_stmt(stmt)?;
    if expr.kind() != "call_expression" {
        return None;
    }

    let function = expr.child_by_field_name("function")?;
    if node_text(function, source) != "drop" {
        return None;
    }

    let args = expr.child_by_field_name("arguments")?;
    // arguments node has children: `(`, ident, `)`
    let arg = args.named_child(0)?;
    if arg.kind() == "identifier" {
        Some(node_text(arg, source).to_string())
    } else {
        None
    }
}

/// Walk `expr_node` looking for `await_expression` nodes. For each found,
/// if there are any live guards, emit a diagnostic.
fn check_await_in_expr(
    expr_node: Node,
    source: &[u8],
    guards: &[GuardBinding],
    diagnostics: &mut Vec<Diagnostic>,
) {
    if guards.is_empty() {
        return;
    }

    visit_awaits(expr_node, source, guards, diagnostics);
}

fn visit_awaits(
    node: Node,
    source: &[u8],
    guards: &[GuardBinding],
    diagnostics: &mut Vec<Diagnostic>,
) {
    if node.kind() == "await_expression" {
        // Only flag awaits that come *after* the guard bindings (by byte offset)
        let await_start = node.start_byte();
        let live_guards: Vec<&GuardBinding> = guards
            .iter()
            .filter(|g| g.start_byte < await_start)
            .collect();

        if !live_guards.is_empty() {
            let guard_names: Vec<&str> = live_guards.iter().map(|g| g.name.as_str()).collect();
            let message = format!(
                "tokio async lock guard{} `{}` {} held across an `.await` point. \
                This can deadlock if another task tries to acquire the same lock. \
                Drop the guard before awaiting, or scope it in a block.",
                if guard_names.len() == 1 { "" } else { "s" },
                guard_names.join("`, `"),
                if guard_names.len() == 1 { "is" } else { "are" },
            );

            diagnostics.push(Diagnostic {
                range: node_to_range(node),
                severity: Some(DiagnosticSeverity::WARNING),
                code: Some(NumberOrString::String(
                    "async-rust/mutex-across-await".to_string(),
                )),
                code_description: None,
                source: Some("async-rust-lsp".to_string()),
                message,
                related_information: None,
                tags: None,
                data: None,
            });
        }

        // Don't recurse into nested awaits — they'll be picked up at their own block level
        return;
    }

    // When we encounter a block node (e.g. if/match/loop body), walk its
    // children sequentially so we can track drop() and shadowing within
    // that branch. This eliminates false positives when a guard is dropped
    // before an await inside the same conditional branch.
    if node.kind() == "block" {
        visit_awaits_in_block(node, source, guards, diagnostics);
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit_awaits(child, source, guards, diagnostics);
    }
}

/// Walk a block node's children in order, tracking `drop()` calls and `let`
/// shadowing against the (outer) guard set. This allows `drop(guard)` inside
/// an `if` branch to kill the guard's liveness for subsequent awaits in that
/// same branch, while leaving other branches unaffected.
fn visit_awaits_in_block(
    block: Node,
    source: &[u8],
    outer_guards: &[GuardBinding],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut guards = outer_guards.to_vec();

    let mut cursor = block.walk();
    for stmt in block.children(&mut cursor) {
        match stmt.kind() {
            "let_declaration" => {
                // Check the RHS for awaits against current guard set
                if let Some(value) = stmt.child_by_field_name("value") {
                    visit_awaits(value, source, &guards, diagnostics);
                }
                // Shadowing kills the old guard
                if let Some(pattern) = stmt.child_by_field_name("pattern") {
                    let name = node_text(pattern, source);
                    guards.retain(|g| g.name != name);
                }
            }
            "expression_statement" | "expression" => {
                // Check for drop() calls that kill guard liveness
                if let Some(dropped) = extract_drop_call(stmt, source) {
                    guards.retain(|g| g.name != dropped);
                }
                // Check for awaits against remaining live guards
                visit_awaits(stmt, source, &guards, diagnostics);
            }
            _ => {
                visit_awaits(stmt, source, &guards, diagnostics);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn node_text<'a>(node: Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

fn node_to_range(node: Node) -> Range {
    let start = node.start_position();
    let end = node.end_position();
    Range {
        start: Position {
            line: start.row as u32,
            character: start.column as u32,
        },
        end: Position {
            line: end.row as u32,
            character: end.column as u32,
        },
    }
}

/// Get the first expression child of a statement node.
fn first_expr_in_stmt(stmt: Node) -> Option<Node> {
    let mut cursor = stmt.walk();
    let result = stmt
        .children(&mut cursor)
        .find(|child| child.is_named() && child.kind() != ";");
    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn diag_messages(source: &str) -> Vec<String> {
        check_mutex_across_await(source)
            .into_iter()
            .map(|d| d.message)
            .collect()
    }

    fn diag_count(source: &str) -> usize {
        check_mutex_across_await(source).len()
    }

    // --- BAD patterns (should produce diagnostics) ---

    #[test]
    fn detects_mutex_guard_across_await() {
        let src = r#"
async fn bad() {
    let mutex = tokio::sync::Mutex::new(0);
    let guard = mutex.lock().await;
    some_future().await;
}
"#;
        assert_eq!(
            diag_count(src),
            1,
            "expected 1 diagnostic, got: {:?}",
            diag_messages(src)
        );
    }

    #[test]
    fn detects_rwlock_write_guard_across_await() {
        let src = r#"
async fn bad() {
    let lock = tokio::sync::RwLock::new(0);
    let guard = lock.write().await;
    some_future().await;
}
"#;
        assert_eq!(
            diag_count(src),
            1,
            "expected 1 diagnostic, got: {:?}",
            diag_messages(src)
        );
    }

    #[test]
    fn detects_rwlock_read_guard_across_await() {
        let src = r#"
async fn bad() {
    let lock = tokio::sync::RwLock::new(0);
    let guard = lock.read().await;
    some_future().await;
}
"#;
        assert_eq!(
            diag_count(src),
            1,
            "expected 1 diagnostic, got: {:?}",
            diag_messages(src)
        );
    }

    #[test]
    fn detects_field_access_lock() {
        let src = r#"
async fn bad(state: &State) {
    let guard = state.data.lock().await;
    do_something().await;
}
"#;
        assert_eq!(diag_count(src), 1);
    }

    #[test]
    fn diagnostic_message_contains_guard_name() {
        let src = r#"
async fn bad() {
    let my_guard = mutex.lock().await;
    something().await;
}
"#;
        let msgs = diag_messages(src);
        assert_eq!(msgs.len(), 1);
        assert!(
            msgs[0].contains("my_guard"),
            "message should name the guard: {}",
            msgs[0]
        );
    }

    #[test]
    fn diagnostic_code_is_correct() {
        let src = r#"
async fn bad() {
    let guard = mutex.lock().await;
    something().await;
}
"#;
        let diags = check_mutex_across_await(src);
        assert_eq!(diags.len(), 1);
        assert_eq!(
            diags[0].code,
            Some(NumberOrString::String(
                "async-rust/mutex-across-await".to_string()
            ))
        );
    }

    #[test]
    fn detects_await_in_let_rhs_with_live_guard() {
        let src = r#"
async fn bad() {
    let guard = m1.lock().await;
    *guard = 1;
    let id = m2.write().await;
    *id = 2;
}
"#;
        assert!(
            diag_count(src) >= 1,
            "expected diagnostic for .await in let RHS while guard is live, got: {:?}",
            diag_messages(src)
        );
    }

    #[test]
    fn detects_await_in_nested_if_block() {
        let src = r#"
async fn bad() {
    let guard = mutex.read().await;
    let snapshot = guard.clone();
    if !snapshot.is_empty() {
        send_frame().await;
    }
}
"#;
        assert_eq!(
            diag_count(src),
            1,
            "expected 1 diagnostic for .await in if body, got: {:?}",
            diag_messages(src)
        );
    }

    #[test]
    fn detects_await_in_nested_loop_block() {
        let src = r#"
async fn bad() {
    let guard = mutex.lock().await;
    loop {
        do_work().await;
        break;
    }
}
"#;
        assert_eq!(
            diag_count(src),
            1,
            "expected 1 diagnostic for .await in loop body, got: {:?}",
            diag_messages(src)
        );
    }

    // --- GOOD patterns (should produce NO diagnostics) ---

    #[test]
    fn no_diagnostic_when_guard_dropped_before_await() {
        let src = r#"
async fn good() {
    let value = {
        let guard = mutex.lock().await;
        *guard
    };
    some_future().await;
}
"#;
        assert_eq!(
            diag_count(src),
            0,
            "should not flag guard dropped before await"
        );
    }

    #[test]
    fn no_diagnostic_when_guard_shadowed_before_await() {
        let src = r#"
async fn good() {
    let guard = mutex.lock().await;
    let value = *guard;
    let guard = 42;
    some_future().await;
}
"#;
        assert_eq!(
            diag_count(src),
            0,
            "should not flag after guard name is shadowed"
        );
    }

    #[test]
    fn no_diagnostic_when_explicit_drop_before_await() {
        let src = r#"
async fn good() {
    let guard = mutex.lock().await;
    let value = *guard;
    drop(guard);
    some_future().await;
}
"#;
        assert_eq!(diag_count(src), 0, "should not flag after explicit drop()");
    }

    #[test]
    fn no_diagnostic_for_std_mutex() {
        // We only care about tokio async locks; std::sync::Mutex.lock() is sync
        // and won't appear as an await_expression on the RHS.
        let src = r#"
async fn fine() {
    let mutex = std::sync::Mutex::new(0);
    let guard = mutex.lock().unwrap();
    some_future().await;
}
"#;
        // std Mutex.lock() is NOT an await_expression, so we don't flag it.
        // (Clippy handles this case; we intentionally leave it to clippy.)
        assert_eq!(diag_count(src), 0);
    }

    #[test]
    fn no_diagnostic_without_subsequent_await() {
        let src = r#"
async fn fine() {
    let guard = mutex.lock().await;
    let value = *guard;
    // no .await after this
}
"#;
        assert_eq!(diag_count(src), 0);
    }

    #[test]
    fn no_diagnostic_for_await_in_if_after_drop() {
        let src = r#"
async fn good() {
    let guard = mutex.lock().await;
    let value = *guard;
    drop(guard);
    if condition {
        some_future().await;
    }
}
"#;
        assert_eq!(
            diag_count(src),
            0,
            "should not flag await in if body after guard is dropped"
        );
    }

    #[test]
    fn no_diagnostic_when_drop_in_same_branch_before_await() {
        let src = r#"
async fn good() {
    let guard = mutex.lock().await;
    let data = guard.clone();
    if condition {
        drop(guard);
        do_async_work(&data).await;
    }
}
"#;
        assert_eq!(
            diag_count(src),
            0,
            "should not flag await after drop() in same branch"
        );
    }

    #[test]
    fn diagnostic_in_else_branch_without_drop() {
        let src = r#"
async fn mixed() {
    let guard = mutex.lock().await;
    if condition {
        drop(guard);
        do_async_work().await;
    } else {
        do_other_work().await;
    }
}
"#;
        let diags = check_mutex_across_await(src);
        assert_eq!(
            diags.len(),
            1,
            "should flag only the else branch: {:?}",
            diag_messages(src)
        );
        // The flagged await should be do_other_work().await in the else branch
        assert!(
            diags[0].message.contains("guard"),
            "diagnostic should name the guard"
        );
    }

    #[test]
    fn no_diagnostic_when_shadowed_in_nested_block() {
        let src = r#"
async fn good() {
    let guard = mutex.lock().await;
    let data = guard.clone();
    if condition {
        let guard = 42;
        do_async_work(&data).await;
    }
}
"#;
        assert_eq!(
            diag_count(src),
            0,
            "should not flag after shadowing in same branch"
        );
    }

    #[test]
    fn no_diagnostic_empty_source() {
        assert_eq!(diag_count(""), 0);
    }

    #[test]
    fn no_diagnostic_non_async_code() {
        let src = r#"
fn sync_fn() {
    let x = 42;
    println!("{}", x);
}
"#;
        assert_eq!(diag_count(src), 0);
    }
}
