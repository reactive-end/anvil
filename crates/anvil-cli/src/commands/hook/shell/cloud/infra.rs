//! Destructive infrastructure-as-code and orchestration CLI rules (Terraform, Pulumi, CDK,
//! Serverless, SAM, Helm, kubectl).

use anvil_core::config::security_guard::GuardCategory;

use super::super::expand::Invocation;
use super::{Parsed, Risk};

const KUBECTL_OPTS: &[&str] = &[
    "-n",
    "--namespace",
    "--context",
    "--kubeconfig",
    "--cluster",
    "--user",
    "-s",
    "--server",
    "--token",
    "--as",
    "--as-group",
    "--request-timeout",
    "--cache-dir",
    "--certificate-authority",
    "--client-certificate",
    "--client-key",
    "--tls-server-name",
    "--username",
    "--password",
    "--profile",
    "--profile-output",
    "-v",
];
const HELM_OPTS: &[&str] = &[
    "-n",
    "--namespace",
    "--kube-context",
    "--kubeconfig",
    "--kube-as-user",
    "--kube-as-group",
    "--kube-apiserver",
    "--kube-token",
    "--kube-ca-file",
    "--registry-config",
    "--repository-config",
    "--repository-cache",
    "--burst-limit",
    "--qps",
];
const PULUMI_OPTS: &[&str] = &["-s", "--stack", "-C", "--cwd", "--config-file", "--color"];
const CDK_OPTS: &[&str] = &[
    "--profile",
    "--app",
    "-a",
    "--context",
    "-c",
    "--output",
    "-o",
    "--region",
    "--role-arn",
    "--require-approval",
];
const SAM_OPTS: &[&str] =
    &["--stack-name", "--region", "--profile", "--config-file", "--config-env", "--template", "-t"];
const SERVERLESS_OPTS: &[&str] =
    &["--stage", "-s", "--region", "-r", "--config", "-c", "--aws-profile", "--profile", "--org"];
const K8S_DELETE: Risk =
    Risk::new(GuardCategory::Infra, "deleting or draining Kubernetes resources");
const HELM_DELETE: Risk = Risk::new(GuardCategory::Infra, "uninstalling a Helm release");
const IAC_DESTROY: Risk = Risk::new(GuardCategory::Infra, "destroying managed infrastructure");
const IAC_STATE: Risk =
    Risk::new(GuardCategory::Infra, "altering or unlocking infrastructure state");
const IAC_APPLY: Risk =
    Risk::new(GuardCategory::Infra, "applying infrastructure changes without confirmation");

fn first_word_is(args: &[String], opts: &[&str], verbs: &[&str]) -> bool {
    Parsed::new(args, opts).words.first().is_some_and(|w| verbs.contains(w))
}

pub(super) fn describe_cdk(args: &[String]) -> Option<Risk> {
    first_word_is(args, CDK_OPTS, &["destroy"]).then_some(IAC_DESTROY)
}

pub(super) fn describe_sam(args: &[String]) -> Option<Risk> {
    first_word_is(args, SAM_OPTS, &["delete"]).then_some(IAC_DESTROY)
}

pub(super) fn describe_serverless(args: &[String]) -> Option<Risk> {
    first_word_is(args, SERVERLESS_OPTS, &["remove"]).then_some(IAC_DESTROY)
}

pub(super) fn describe_kubectl(args: &[String]) -> Option<Risk> {
    let p = Parsed::new(args, KUBECTL_OPTS);
    let destructive = match p.word(0) {
        "delete" | "drain" => !p.dry_run(),
        "replace" => p.has_flag(&["force"]),
        _ => false,
    };
    destructive.then_some(K8S_DELETE)
}

pub(super) fn describe_helm(args: &[String]) -> Option<Risk> {
    let p = Parsed::new(args, HELM_OPTS);
    (matches!(p.word(0), "uninstall" | "delete" | "del" | "un") && !p.dry_run())
        .then_some(HELM_DELETE)
}

pub(super) fn describe_terraform(args: &[String]) -> Option<Risk> {
    let mut p = Parsed::new(args, &[]);
    let lead = p.words.iter().take_while(|w| matches!(**w, "run-all" | "run")).count();
    p.words.drain(..lead);
    match p.word(0) {
        "destroy" => Some(IAC_DESTROY),
        "apply" if p.has_flag(&["destroy"]) => Some(IAC_DESTROY),
        "apply" if p.has_flag(&["auto-approve"]) => Some(IAC_APPLY),
        "force-unlock" => Some(IAC_STATE),
        "state" if p.word(1) == "rm" => Some(IAC_STATE),
        "workspace" if p.word(1) == "delete" => Some(IAC_STATE),
        _ => None,
    }
}

pub(super) fn describe_pulumi(args: &[String]) -> Option<Risk> {
    let p = Parsed::new(args, PULUMI_OPTS);
    let destructive =
        p.word(0) == "destroy" || (p.word(0) == "stack" && matches!(p.word(1), "rm" | "remove"));
    destructive.then_some(IAC_DESTROY)
}

/// `sls` is also the PowerShell alias of `Select-String`, so only a bare `sls remove` counts.
pub(super) fn describe_sls(inv: &Invocation) -> Option<Risk> {
    let p = Parsed::new(&inv.args, SERVERLESS_OPTS);
    (!inv.fed_by_pipe && p.words == ["remove"]).then_some(IAC_DESTROY)
}
