/// Integration tests for async-rust-lsp rule engine.
///
/// These tests load fixture files from `tests/fixtures/` and verify that the
/// rule engine produces the expected diagnostics.
use async_rust_lsp::rules::cancel_unsafe_in_select::check_cancel_unsafe_in_select;
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
            Some(NumberOrString::String(
                "async-rust/mutex-across-await".to_string()
            )),
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
        diags
            .iter()
            .map(|d| (&d.range, &d.message))
            .collect::<Vec<_>>()
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
    assert_eq!(
        diags[0].range.start.line, 3,
        "diagnostic should be on line 3"
    );
}

// ---------------------------------------------------------------------------
// cancel-unsafe-in-select fixtures
// ---------------------------------------------------------------------------

#[test]
fn fixture_bad_cancel_unsafe_produces_diagnostics() {
    let source = include_str!("fixtures/bad_cancel_unsafe_in_select.rs");
    let diags = check_cancel_unsafe_in_select(source);
    assert!(
        !diags.is_empty(),
        "Expected diagnostics for bad_cancel_unsafe_in_select.rs, got none"
    );
}

#[test]
fn fixture_bad_cancel_unsafe_flags_every_arm() {
    let source = include_str!("fixtures/bad_cancel_unsafe_in_select.rs");
    let diags = check_cancel_unsafe_in_select(source);
    // The fixture has 10 cancel-unsafe call sites across 9 functions
    // (one fn has two unsafe arms in the same select!). Verify we catch
    // all of them.
    assert_eq!(
        diags.len(),
        10,
        "expected 10 diagnostics, got {}: {:?}",
        diags.len(),
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

#[test]
fn fixture_bad_cancel_unsafe_diagnostic_code() {
    use tower_lsp::lsp_types::NumberOrString;
    let source = include_str!("fixtures/bad_cancel_unsafe_in_select.rs");
    let diags = check_cancel_unsafe_in_select(source);
    for diag in &diags {
        assert_eq!(
            diag.code,
            Some(NumberOrString::String(
                "async-rust/cancel-unsafe-in-select".to_string()
            )),
        );
    }
}

#[test]
fn fixture_bad_cancel_unsafe_all_warnings() {
    use tower_lsp::lsp_types::DiagnosticSeverity;
    let source = include_str!("fixtures/bad_cancel_unsafe_in_select.rs");
    let diags = check_cancel_unsafe_in_select(source);
    for diag in &diags {
        assert_eq!(
            diag.severity,
            Some(DiagnosticSeverity::WARNING),
            "all diagnostics should be WARNING severity"
        );
    }
}

#[test]
fn fixture_good_cancel_safe_produces_no_diagnostics() {
    let source = include_str!("fixtures/good_cancel_safe_in_select.rs");
    let diags = check_cancel_unsafe_in_select(source);
    assert!(
        diags.is_empty(),
        "Expected no diagnostics for good_cancel_safe_in_select.rs, got: {:#?}",
        diags
            .iter()
            .map(|d| (&d.range, &d.message))
            .collect::<Vec<_>>()
    );
}

#[test]
fn cancel_unsafe_does_not_flag_mutex_fixtures() {
    // Sanity check: the mutex-rule fixtures must not trigger the
    // cancel-unsafe rule (and vice versa, covered by the existing
    // mutex tests above).
    let bad_mutex = include_str!("fixtures/bad_mutex_across_await.rs");
    let good_mutex = include_str!("fixtures/good_no_mutex_across_await.rs");
    assert!(
        check_cancel_unsafe_in_select(bad_mutex).is_empty(),
        "cancel-unsafe rule should not fire on the mutex bad fixture"
    );
    assert!(
        check_cancel_unsafe_in_select(good_mutex).is_empty(),
        "cancel-unsafe rule should not fire on the mutex good fixture"
    );
}

#[test]
fn mutex_rule_does_not_flag_cancel_unsafe_fixtures() {
    let bad = include_str!("fixtures/bad_cancel_unsafe_in_select.rs");
    let good = include_str!("fixtures/good_cancel_safe_in_select.rs");
    assert!(
        check_mutex_across_await(bad).is_empty(),
        "mutex rule should not fire on the cancel-unsafe bad fixture"
    );
    assert!(
        check_mutex_across_await(good).is_empty(),
        "mutex rule should not fire on the cancel-unsafe good fixture"
    );
}
