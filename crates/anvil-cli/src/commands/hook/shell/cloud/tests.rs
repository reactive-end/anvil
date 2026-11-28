use super::*;
use crate::commands::hook::shell::expand::expand;

fn risk(cmd: &str) -> Option<Risk> {
    expand(cmd).invocations.iter().find_map(describe)
}

fn assert_risk(category: GuardCategory, cmds: &[&str]) {
    let wrong: Vec<_> =
        cmds.iter().filter(|c| risk(c).map(|r| r.category) != Some(category)).collect();
    assert!(wrong.is_empty(), "expected {category:?} for: {wrong:#?}");
}

#[test]
fn flags_destructive_aws_commands() {
    assert_risk(
        GuardCategory::Cloud,
        &[
            "aws ec2 terminate-instances --instance-ids i-123",
            "aws --profile prod --region us-east-1 ec2 terminate-instances --instance-ids i-1",
            "aws --output=json rds delete-db-instance --db-instance-identifier x",
            "aws s3 rb s3://bucket --force",
            "aws s3 rm s3://bucket/key",
            "aws s3 rm s3://bucket --recursive",
            "aws s3 sync . s3://bucket --delete",
            "aws s3api delete-bucket --bucket b",
            "aws cloudformation delete-stack --stack-name s",
            "aws ec2 deregister-image --image-id ami-1",
            "aws sqs purge-queue --queue-url u",
            "aws iam remove-user-from-group --user-name u --group-name g",
            "sudo aws ec2 terminate-instances --instance-ids i-1",
            "bash -c \"aws ec2 terminate-instances --instance-ids i-1\"",
        ],
    );
}

#[test]
fn flags_gcloud_gsutil_and_az_deletions() {
    assert_risk(
        GuardCategory::Cloud,
        &[
            "gcloud compute instances delete vm-1 --zone us-central1-a",
            "gcloud --project p sql instances delete db",
            "gcloud projects delete my-project",
            "gcloud storage rm gs://b/o",
            "gcloud storage buckets delete gs://b",
            "gsutil rm -r gs://b",
            "gsutil -m rm -r gs://b/**",
            "gsutil rb gs://b",
            "az group delete --name rg --yes",
            "az --subscription s vm delete -n vm -g rg",
            "az keyvault purge --name kv",
            "az storage blob delete-batch --source c",
        ],
    );
}

#[test]
fn flags_infrastructure_tooling() {
    assert_risk(
        GuardCategory::Infra,
        &[
            "kubectl delete pod web-1",
            "kubectl -n prod --context eks delete deployment api",
            "kubectl drain node-1 --ignore-daemonsets",
            "kubectl replace --force -f pod.yaml",
            "helm uninstall release -n prod",
            "helm delete release",
            "terraform destroy",
            "terraform -chdir=infra destroy -auto-approve",
            "tofu destroy",
            "terragrunt run-all destroy",
            "terraform apply -destroy",
            "terraform apply -auto-approve",
            "terraform state rm aws_instance.a",
            "terraform workspace delete dev",
            "terraform force-unlock abc",
            "pulumi destroy --yes",
            "pulumi stack rm dev",
            "cdk destroy --all",
            "serverless remove --stage prod",
            "sls remove --stage prod",
            "sam delete --stack-name s",
            "bash -c \"terraform destroy\"",
            "sudo -u deploy kubectl delete ns prod",
        ],
    );
}

#[test]
fn flags_destructive_container_commands() {
    assert_risk(
        GuardCategory::Containers,
        &[
            "docker system prune -af",
            "docker volume prune -f",
            "docker volume rm data",
            "docker image prune -a",
            "docker container prune -f",
            "docker rm -f a b c",
            "docker rm -f $(docker ps -aq)",
            "docker rmi -f img1 img2",
            "docker compose down -v",
            "docker compose -f dev.yml down --volumes",
            "docker-compose down -v",
            "podman system prune --all",
            "podman volume rm data",
            "podman compose down -v",
        ],
    );
}

#[test]
fn ignores_read_only_and_dry_run_commands() {
    let noisy: Vec<_> = [
        "aws s3 ls",
        "aws s3 cp delete-me.txt s3://bucket/",
        "aws ec2 describe-instances --profile prod",
        "aws ec2 terminate-instances --instance-ids i-1 --dry-run",
        "gcloud compute instances list",
        "gsutil ls gs://b",
        "az group list",
        "az vm show -n delete -g rg",
        "kubectl get pods -n prod",
        "kubectl delete pod x --dry-run=client",
        "kubectl apply -f deploy.yaml",
        "helm list",
        "helm install r chart",
        "terraform plan",
        "terraform plan -destroy",
        "terraform apply",
        "terraform state list",
        "pulumi up",
        "cdk deploy",
        "sam deploy",
        "sls deploy",
        "sls remove file.txt",
        "docker ps",
        "docker run --rm app",
        "docker rm web",
        "docker rm -f web",
        "docker image prune",
        "docker compose up -d",
        "docker compose down",
        "podman ps",
    ]
    .iter()
    .filter_map(|cmd| risk(cmd).map(|r| (cmd, r.desc)))
    .collect();
    assert!(noisy.is_empty(), "expected routine commands to pass: {noisy:#?}");
}
