//! `anvil circuit`: inspect or reset the N=3 Circuit Breaker.

use std::path::Path;

use anyhow::Result;
use anvil_core::circuit_breaker::CircuitBreaker;
use anvil_core::config::AnvilConfig;

use crate::cli::subcmds::CircuitSubcmd;

/// Executes `anvil circuit status|reset`.
pub fn execute(root: &Path, sub: CircuitSubcmd) -> Result<()> {
    let cfg = AnvilConfig::load_or_default(root)?;
    let cb = CircuitBreaker::new(root, cfg.circuit_breaker_limit);

    match sub {
        CircuitSubcmd::Status => {
            let st = cb.status()?;
            println!(
                "circuit: {:?} (consecutive_failures={}/{})",
                st.state, st.consecutive_failures, st.limit
            );
            if let Some(esc) = st.escalation {
                println!(
                    "escalation: tool={} fp={} cmd=`{}`",
                    esc.tool, esc.fingerprint, esc.last_command
                );
                println!("action: {}", esc.recommended_action);
            }
        }
        CircuitSubcmd::Reset => {
            let st = cb.reset()?;
            let _ = anvil_adapters::inbox::remove_source(root, "circuit-breaker");
            println!("circuit reset: {:?} (limit={})", st.state, st.limit);
        }
    }
    Ok(())
}
