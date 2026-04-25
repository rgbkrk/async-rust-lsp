use async_rust_lsp::config::Config;
use async_rust_lsp::rules::cancel_unsafe_in_select::check_cancel_unsafe_in_select_with;
use std::path::Path;

fn main() {
    let path = std::env::args().nth(1).expect("usage: probe <file>");
    let p = Path::new(&path);
    let (cfg, _root) = Config::discover_from(p.parent().unwrap_or(p));
    let extras = &cfg.rules.cancel_unsafe_in_select.extra;
    println!("extras loaded: {:?}", extras);
    let src = std::fs::read_to_string(&path).unwrap();
    let diags = check_cancel_unsafe_in_select_with(&src, extras);
    println!("{} diagnostic(s)", diags.len());
    for d in &diags {
        println!(
            "  line {}: {}",
            d.range.start.line + 1,
            d.message.lines().next().unwrap()
        );
    }
}
