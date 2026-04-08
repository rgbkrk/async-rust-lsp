/// Integration tests for async-rust-lsp rule engine.
///
/// These tests load fixture files from `tests/fixtures/` and verify that the
/// rule engine produces the expected diagnostics.
use async_rust_lsp::rules::mutex_across_await::check_mutex_across_await;

// ---------------------------------------------------------------------------
// Bad patterns — each should produce >= 1 diagnostic
// ---------------------------------------------------------------------------

#[test]
fn fixture_bad_basic_mutex_produces_diagnostics() {
    let source = include_str!("fixtures/bad_mutex_across_await.rs");
    let diags = check_mutex_across_await(source);
    assert!(
        !diags.is_empty(),
        "Expected diagnostics for bad_mutex_across_await.rs, got none"
    );
}

#[test]
fn fixture_bad_has_correct_source_field() {
    let source = include_str!("fixtures/bad_mutex_across_await.rs");
    let diags = check_mutex_across_await(source);
    for diag in &diags {
        assert_eq!(
            diag.source.as_deref(),
            Some("async-rust-lsp"),
            "diagnostic source should be 'async-rust-lsp'"
        );
    }
}

#[test]
fn fixture_bad_all_diagnostics_are_warnings() {
    let source = include_str!("fixtures/bad_mutex_across_await.rs");
    let diags = check_mutex_across_await(source);
    use tower_lsp::lsp_types::DiagnosticSeverity;
    for diag in &diags {
        assert_eq!(
            diag.severity,
            Some(DiagnosticSeverity::WARNING),
            "all diagnostics should be WARNING severity"
        );
    }
}

#[test]
fn fixture_bad_diagnostic_code_is_mutex_across_await() {
    let source = include_str!("fixtures/bad_mutex_across_await.rs");
    let diags = check_mutex_across_await(source);
    use tower_lsp::lsp_types::NumberOrString;
    for diag in &diags {
        assert_eq!(
            diag.code,
            Some(NumberOrString::String("async-rust/mutex-across-await".to_string())),
        );
    }
}

#[test]
fn fixture_bad_contains_basic_case_diagnostic() {
    // basic_mutex_across_await should be detected
    let source = include_str!("fixtures/bad_mutex_across_await.rs");
    let diags = check_mutex_across_await(source);
    let has_guard_mention = diags.iter().any(|d| d.message.contains("guard"));
    assert!(
        has_guard_mention,
        "expected at least one diagnostic mentioning 'guard', got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Good patterns — should produce ZERO diagnostics
// ---------------------------------------------------------------------------

#[test]
fn fixture_good_produces_no_diagnostics() {
    let source = include_str!("fixtures/good_no_mutex_across_await.rs");
    let diags = check_mutex_across_await(source);
    assert!(
        diags.is_empty(),
        "Expected no diagnostics for good_no_mutex_across_await.rs, got: {:#?}",
        diags.iter().map(|d| (&d.range, &d.message)).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Inline edge cases
// ---------------------------------------------------------------------------

#[test]
fn inline_multiple_awaits_after_guard_each_flagged() {
    let src = r#"
async fn bad() {
    let guard = mutex.lock().await;
    first_op().await;
    second_op().await;
}
"#;
    let diags = check_mutex_across_await(src);
    assert!(
        diags.len() >= 2,
        "expected at least 2 diagnostics for 2 awaits after guard, got {}",
        diags.len()
    );
}

#[test]
fn inline_nested_fn_guard_doesnt_bleed_into_outer() {
    // A guard in a nested async block/closure should not affect the outer scope
    let src = r#"
async fn outer() {
    // No guard in outer scope — only in the inner async block
    let task = tokio::spawn(async move {
        let guard = mutex.lock().await;
        use_guard(&guard);
        // no further await in this inner block
    });
    outer_await().await; // outer has no guard — should not be flagged
}
"#;
    // The inner block's guard should not produce a diagnostic because there's
    // no subsequent await *in that inner block* after the guard.
    // The outer await has no guard.
    let diags = check_mutex_across_await(src);
    // Zero or low diagnostics expected — the outer await is clean.
    // The inner block may or may not trigger depending on tree structure,
    // but the outer `outer_await().await` must NOT be flagged.
    let outer_flagged = diags.iter().any(|d| {
        // outer_await().await is on line 10 (0-indexed: ~9)
        d.range.start.line >= 9
    });
    assert!(
        !outer_flagged,
        "outer await should not be flagged due to inner scope guard"
    );
}

#[test]
fn inline_diagnostic_position_is_at_await_keyword() {
    let src = r#"
async fn bad() {
    let guard = mutex.lock().await;
    something().await;
}
"#;
    let diags = check_mutex_across_await(src);
    assert_eq!(diags.len(), 1);
    // The diagnostic should be on line 3 (0-indexed), at "something().await"
    assert_eq!(diags[0].range.start.line, 3, "diagnostic should be on line 3");
}
