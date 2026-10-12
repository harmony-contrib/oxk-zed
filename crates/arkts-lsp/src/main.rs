#[global_allocator]
static ALLOCATOR: mimalloc_safe::MiMalloc = mimalloc_safe::MiMalloc;

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.iter().any(|s| s == "--version") {
        println!(
            "arkts-lsp {} (tsrs {})",
            env!("CARGO_PKG_VERSION"),
            tsrs_core::version()
        );
        return;
    }
    if args
        .iter()
        .any(|s| !matches!(s.as_str(), "--stdio" | "-stdio" | "--lsp"))
    {
        eprintln!("usage: arkts-lsp [--stdio] [--version]");
        std::process::exit(2);
    }
    if let Err(error) = arkts_lsp::run_stdio() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
