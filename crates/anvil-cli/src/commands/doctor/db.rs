//! Doctor check that Google MCP Toolbox is installed when the project uses it.

use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_qa::api::db::{DOWNLOAD_HINT, ToolboxClient};

use crate::ui;

/// Reports the Toolbox binary when `db` is configured or a tools file exists; a missing binary
/// is a warning, since only database checks need it.
pub(super) fn check(root: &Path, config: &AnvilConfig) {
    if config.db.is_default() && !config.db.config_path(root).is_file() {
        return;
    }
    let client = ToolboxClient::new(root, &config.db);
    match client.version() {
        Ok(version) => ui::print_status_row(&ui::ok_badge(), "toolbox", &version),
        Err(_) => ui::print_status_row(
            &ui::warn_badge(),
            "toolbox",
            &format!(
                "`{}` not found; download it from {DOWNLOAD_HINT} and set `db.toolbox`",
                client.program().display()
            ),
        ),
    }
}
