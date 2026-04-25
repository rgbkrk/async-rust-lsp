use async_rust_lsp::rules::cancel_unsafe_in_select::check_cancel_unsafe_in_select;
fn main() {
    let path = std::env::args().nth(1).expect("usage: probe <file>");
    let src = std::fs::read_to_string(&path).unwrap();
    let diags = check_cancel_unsafe_in_select(&src);
    println!("{}: {} diagnostic(s)", path, diags.len());
    for d in &diags {
        println!(
            "  line {}: {}",
            d.range.start.line + 1,
            d.message.lines().next().unwrap()
        );
    }
}
