//! Env files, environment variables and required paths.

use anvil_core::config::prereqs::{EnvFilePrereq, EnvVarPrereq, PathPrereq};

use super::{Ctx, failure_status};
use crate::envfile::keys_of_file;
use crate::host::Host;
use crate::report::{Item, Kind, Status};

/// Every key of the example must exist in the real file. Only key names are ever reported.
pub(super) fn check_env_files(ctx: &Ctx<'_>, pairs: &[EnvFilePrereq]) -> Vec<Item> {
    pairs.iter().map(|pair| check_pair(ctx, pair)).collect()
}

fn check_pair(ctx: &Ctx<'_>, pair: &EnvFilePrereq) -> Item {
    let mut item = Item::new(pair.subrepo.as_deref(), Kind::EnvFile, &pair.file, Status::Ok)
        .source(&pair.example);
    let Some(wanted) = keys_of_file(&ctx.root.join(&pair.example)) else {
        item.status = Status::Warning;
        return item.message(format!("cannot read the example file {}", pair.example));
    };
    let Some(have) = keys_of_file(&ctx.root.join(&pair.file)) else {
        item.status = Status::Missing;
        return item
            .message("file not found")
            .hint(&format!("copy {} to {} and fill in the values", pair.example, pair.file));
    };
    let missing: Vec<&String> = wanted.iter().filter(|k| !have.contains(k)).collect();
    if missing.is_empty() {
        return item.message(format!("all {} keys present", wanted.len()));
    }
    item.status = Status::Missing;
    let names: Vec<&str> = missing.iter().map(|k| k.as_str()).collect();
    item.message(format!("missing keys: {}", names.join(", ")))
        .hint(&format!("add them to {} (see {})", pair.file, pair.example))
}

/// A variable counts as set when it is in the environment or defined in the project's `.env`.
pub(super) fn check_env_vars(host: &dyn Host, ctx: &Ctx<'_>, vars: &[EnvVarPrereq]) -> Vec<Item> {
    vars.iter()
        .map(|var| {
            let mut item = Item::new(var.subrepo.as_deref(), Kind::EnvVar, &var.name, Status::Ok)
                .source(&var.source)
                .required(var.required)
                .hint(&var.hint);
            if host.env_var(&var.name).is_some() {
                return item.message("set in the environment");
            }
            let dotenv = ctx.dir_of(var.subrepo.as_deref()).join(".env");
            if keys_of_file(&dotenv).is_some_and(|keys| keys.contains(&var.name)) {
                return item.message("defined in .env");
            }
            item.status = failure_status(var.required, Status::Missing);
            item.message("not set in the environment or in .env")
        })
        .collect()
}

pub(super) fn check_paths(ctx: &Ctx<'_>, paths: &[PathPrereq]) -> Vec<Item> {
    paths
        .iter()
        .map(|path| {
            let mut item = Item::new(path.subrepo.as_deref(), Kind::Path, &path.path, Status::Ok)
                .source(&path.source)
                .required(path.required)
                .hint(&path.hint);
            if !ctx.root.join(&path.path).exists() {
                item.status = failure_status(path.required, Status::Missing);
                item = item.message("does not exist");
            }
            item
        })
        .collect()
}
