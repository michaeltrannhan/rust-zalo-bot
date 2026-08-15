//! CLI entrypoint for the zl-expense binary.

use clap::Parser;
use zl_expense::{Cli, execute};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = execute(cli).await;
    std::process::exit(code.as_i32());
}
