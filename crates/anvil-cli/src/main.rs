//! Thin CLI entry point for the `anvil` engineering harness.

use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "anvil", version, about = "Anvil AI engineering harness")]
struct Cli {
    /// Print workspace crate wiring summary.
    #[arg(long)]
    info: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if cli.info {
        let _ = anvil_core::traits::Tier::Tier0;
        println!(
            "anvil workspace ready: {}, {}, {}, {}, {}, {}, {}, {}",
            anvil_ast::crate_name(),
            anvil_graph::crate_name(),
            anvil_memory::crate_name(),
            anvil_mcp::crate_name(),
            anvil_tasks::crate_name(),
            anvil_adapters::crate_name(),
            anvil_deps::crate_name(),
            anvil_qa::crate_name(),
        );
    }
    Ok(())
}
