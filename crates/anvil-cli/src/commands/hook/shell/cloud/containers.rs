//! Destructive container CLI rules (Docker, Podman, Compose).

use anvil_core::config::security_guard::GuardCategory;

use super::{Parsed, Risk};

const DOCKER_OPTS: &[&str] = &[
    "--context",
    "-c",
    "-H",
    "--host",
    "--config",
    "--log-level",
    "-l",
    "--tlscacert",
    "--tlscert",
    "--tlskey",
];
const COMPOSE_OPTS: &[&str] = &[
    "--context",
    "-H",
    "--host",
    "--config",
    "-f",
    "--file",
    "-p",
    "--project-name",
    "--project-directory",
    "--profile",
    "--env-file",
    "--ansi",
    "--parallel",
];

const CONTAINER_PRUNE: Risk =
    Risk::new(GuardCategory::Containers, "pruning or removing container data and volumes");
const CONTAINER_FORCE_RM: Risk =
    Risk::new(GuardCategory::Containers, "force-removing containers or images");

pub(super) fn describe_docker(args: &[String]) -> Option<Risk> {
    let p = Parsed::new(args, DOCKER_OPTS);
    match p.word(0) {
        "compose" => describe_compose(&Parsed::new(args, COMPOSE_OPTS), 1),
        "system" | "container" if p.word(1) == "prune" => Some(CONTAINER_PRUNE),
        "volume" if matches!(p.word(1), "prune" | "rm" | "remove") => Some(CONTAINER_PRUNE),
        "image" if p.word(1) == "prune" && p.has_flag(&["a", "all"]) => Some(CONTAINER_PRUNE),
        "rm" | "rmi" => force_removes(&p, 1),
        "container" | "image" if matches!(p.word(1), "rm" | "remove") => force_removes(&p, 2),
        _ => None,
    }
}

/// Flags forced removal of several targets or of a command substitution result.
fn force_removes(p: &Parsed, first_target: usize) -> Option<Risk> {
    let targets = &p.words[first_target.min(p.words.len())..];
    // The lexer leaves an empty word where a `$(...)` substitution stood.
    let dynamic = targets.iter().any(|t| t.is_empty() || t.contains('$') || t.contains('`'));
    (p.has_flag(&["f", "force"]) && (targets.len() > 1 || dynamic)).then_some(CONTAINER_FORCE_RM)
}

/// `compose` words start at `sub_idx`: `docker compose down -v` or `docker-compose down -v`.
fn describe_compose(p: &Parsed, sub_idx: usize) -> Option<Risk> {
    let drops_volumes = p.has_flag(&["v", "volumes"]);
    (p.word(sub_idx) == "down" && drops_volumes).then_some(CONTAINER_PRUNE)
}

/// `docker-compose` and `podman-compose` binaries: the sub-command is the first word.
pub(super) fn describe_standalone_compose(args: &[String]) -> Option<Risk> {
    describe_compose(&Parsed::new(args, COMPOSE_OPTS), 0)
}
