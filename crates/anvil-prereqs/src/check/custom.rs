//! User-defined checks: a shell command that must exit 0.

use std::thread;

use anvil_core::config::prereqs::CustomPrereq;

use super::{Ctx, failure_status};
use crate::host::{Host, RunOutcome};
use crate::report::{Item, Kind, Status};

pub(super) fn check_custom(host: &dyn Host, ctx: &Ctx<'_>, custom: &[CustomPrereq]) -> Vec<Item> {
    thread::scope(|scope| {
        let handles: Vec<_> =
            custom.iter().map(|c| scope.spawn(move || run_one(host, ctx, c))).collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    })
}

fn run_one(host: &dyn Host, ctx: &Ctx<'_>, check: &CustomPrereq) -> Item {
    let item = Item::new(check.subrepo.as_deref(), Kind::Custom, &check.name, Status::Ok)
        .source(&check.command)
        .required(check.required)
        .hint(&check.hint);
    let bad = failure_status(check.required, Status::Missing);
    match host.run_shell(&check.command, ctx.opts.custom_timeout) {
        RunOutcome::Ran { code: 0, .. } => item,
        RunOutcome::Ran { code, .. } => failed(item, bad, format!("exited with status {code}")),
        RunOutcome::TimedOut => {
            failed(item, bad, format!("timed out after {}s", ctx.opts.custom_timeout.as_secs()))
        }
        RunOutcome::NotFound => failed(item, bad, "the shell could not run the command".to_owned()),
    }
}

fn failed(mut item: Item, status: Status, message: String) -> Item {
    item.status = status;
    item.message(message)
}
