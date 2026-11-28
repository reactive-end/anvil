//! Destructive cloud CLI rules (AWS, Google Cloud, Azure) over expanded invocations, plus the
//! shared argument parser and dispatch to the infrastructure and container rule sets.

mod containers;
mod infra;
#[cfg(test)]
mod tests;

use anvil_core::config::security_guard::GuardCategory;

use super::Risk;
use super::expand::Invocation;

const AWS_OPTS: &[&str] = &[
    "--profile",
    "--region",
    "--output",
    "--endpoint-url",
    "--query",
    "--ca-bundle",
    "--color",
    "--cli-read-timeout",
    "--cli-connect-timeout",
];
const AWS_DELETE_PREFIXES: &[&str] =
    &["delete-", "batch-delete", "terminate-", "remove-", "deregister-", "purge-", "destroy"];
const GCLOUD_OPTS: &[&str] = &[
    "--project",
    "--account",
    "--configuration",
    "--format",
    "--filter",
    "--impersonate-service-account",
    "--billing-project",
    "--verbosity",
];
const AZ_OPTS: &[&str] =
    &["--subscription", "--output", "-o", "--query", "--resource-group", "-g", "--name", "-n"];
const AWS_DELETE: Risk = Risk::new(GuardCategory::Cloud, "deleting or terminating AWS resources");
const AWS_S3: Risk = Risk::new(GuardCategory::Cloud, "deleting S3 buckets or objects");
const GCP_DELETE: Risk = Risk::new(GuardCategory::Cloud, "deleting Google Cloud resources");
const AZ_DELETE: Risk = Risk::new(GuardCategory::Cloud, "deleting or purging Azure resources");

/// Positional words and flags of a command line, with the values of known options skipped.
struct Parsed<'a> {
    words: Vec<&'a str>,
    flags: Vec<&'a str>,
}

impl<'a> Parsed<'a> {
    fn new(args: &'a [String], value_opts: &[&str]) -> Self {
        let mut parsed = Self { words: Vec::new(), flags: Vec::new() };
        let mut iter = args.iter().map(String::as_str);
        while let Some(arg) = iter.next() {
            if arg.len() > 1 && arg.starts_with('-') {
                parsed.flags.push(arg);
                if value_opts.contains(&arg) {
                    iter.next();
                }
            } else {
                parsed.words.push(arg);
            }
        }
        parsed
    }

    fn word(&self, idx: usize) -> &str {
        self.words.get(idx).copied().unwrap_or_default()
    }

    /// Matches `names` against flags ignoring dashes and `=value`; one-letter names also match
    /// inside short clusters such as `-fv`. `--flag=false` does not count as set.
    fn has_flag(&self, names: &[&str]) -> bool {
        self.flags.iter().any(|raw| {
            let trimmed = raw.trim_start_matches('-');
            let (name, value) = trimmed.split_once('=').unwrap_or((trimmed, ""));
            let cluster = !raw.starts_with("--")
                && name.len() > 1
                && name.chars().all(|c| c.is_ascii_alphabetic());
            let hit =
                names.iter().any(|n| *n == name || (n.len() == 1 && cluster && name.contains(n)));
            hit && value != "false"
        })
    }

    fn dry_run(&self) -> bool {
        self.flags.iter().any(|f| {
            let dry = f.starts_with("--dry-run") && !f.ends_with("=none") && !f.ends_with("=false");
            dry || *f == "--dryrun"
        })
    }
}

/// Describes destructive cloud, infrastructure, and container commands, or `None` for routine ones.
pub fn describe(inv: &Invocation) -> Option<Risk> {
    let args = inv.args.as_slice();
    match inv.name.as_str() {
        "aws" => describe_aws(args),
        "gcloud" => describe_gcloud(args),
        "gsutil" => describe_gsutil(args),
        "az" => describe_az(args),
        "kubectl" => infra::describe_kubectl(args),
        "helm" => infra::describe_helm(args),
        "terraform" | "tofu" | "terragrunt" => infra::describe_terraform(args),
        "pulumi" => infra::describe_pulumi(args),
        "cdk" => infra::describe_cdk(args),
        "sam" => infra::describe_sam(args),
        "serverless" => infra::describe_serverless(args),
        "sls" => infra::describe_sls(inv),
        "docker" | "podman" => containers::describe_docker(args),
        "docker-compose" | "podman-compose" => containers::describe_standalone_compose(args),
        _ => None,
    }
}

fn describe_aws(args: &[String]) -> Option<Risk> {
    let p = Parsed::new(args, AWS_OPTS);
    if p.dry_run() {
        return None;
    }
    let (service, op) = (p.word(0), p.word(1));
    if AWS_DELETE_PREFIXES.iter().any(|prefix| op.starts_with(prefix)) {
        return Some(AWS_DELETE);
    }
    let s3_delete = match op {
        "rb" | "rm" => true,
        "sync" => p.has_flag(&["delete"]),
        _ => false,
    };
    (service == "s3" && s3_delete).then_some(AWS_S3)
}

fn describe_gcloud(args: &[String]) -> Option<Risk> {
    let p = Parsed::new(args, GCLOUD_OPTS);
    let path = &p.words[..p.words.len().min(6)];
    let deletes = path.contains(&"delete")
        || path.windows(2).any(|pair| pair[0] == "storage" && pair[1] == "rm");
    deletes.then_some(GCP_DELETE)
}

fn describe_gsutil(args: &[String]) -> Option<Risk> {
    let p = Parsed::new(args, &["-o", "-h", "-u"]);
    let deletes = matches!(p.word(0), "rm" | "rb") || (p.word(0) == "rsync" && p.has_flag(&["d"]));
    deletes.then_some(GCP_DELETE)
}

fn describe_az(args: &[String]) -> Option<Risk> {
    let p = Parsed::new(args, AZ_OPTS);
    let path = &p.words[..p.words.len().min(6)];
    path.iter().any(|w| matches!(*w, "delete" | "purge" | "delete-batch")).then_some(AZ_DELETE)
}
