//! UCC CLI executable entry point.

#[tokio::main]
async fn main() {
    let code = ucc_cli::run_cli(std::env::args_os()).await;
    std::process::exit(code);
}
