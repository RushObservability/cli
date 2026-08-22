use std::io::{self, Write};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::{
    cli::{Cli, CredentialArgs, KubeconfigArgs, KubernetesArgs, KubernetesCommand},
    config,
};

pub fn run(cli: &Cli, args: &KubernetesArgs) -> Result<()> {
    match &args.command {
        KubernetesCommand::Kubeconfig(args) => print_kubeconfig(cli, args),
        KubernetesCommand::Credential(args) => print_credential(cli, args),
    }
}

fn print_kubeconfig(cli: &Cli, args: &KubeconfigArgs) -> Result<()> {
    validate_cluster(&args.cluster)?;
    let gateway_url = config::kubernetes_gateway_url(cli, args.gateway_url.clone())?;
    let document = kubeconfig(cli, args, &gateway_url);
    write_json(&document)
}

fn print_credential(cli: &Cli, args: &CredentialArgs) -> Result<()> {
    validate_cluster(&args.cluster)?;
    let token = config::api_key(cli)?
        .context("missing Rush API key; set RUSH_API_KEY or api_key in the Rush config file")?;
    write_json(&exec_credential(&token))
}

fn validate_cluster(cluster: &str) -> Result<()> {
    if cluster.trim().is_empty() {
        bail!("cluster identifier cannot be empty")
    }
    if cluster.len() > 256 || cluster.chars().any(char::is_control) {
        bail!("cluster identifier must be 256 bytes or fewer and contain no control characters")
    }
    Ok(())
}

fn write_json(value: &Value) -> Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer_pretty(&mut output, value)?;
    output.write_all(b"\n")?;
    Ok(())
}

fn kubeconfig(cli: &Cli, args: &KubeconfigArgs, gateway_url: &str) -> Value {
    let context_name = args
        .context
        .clone()
        .unwrap_or_else(|| format!("rush-{}", args.cluster));
    let user_name = format!("rush-{}", args.cluster);
    let mut exec_args = Vec::new();
    if let Some(path) = cli.config.as_ref() {
        exec_args.extend(["--config".to_string(), path.to_string_lossy().into_owned()]);
    }
    exec_args.extend([
        "kubernetes".to_string(),
        "credential".to_string(),
        "--cluster".to_string(),
        args.cluster.clone(),
    ]);

    let mut cluster = json!({ "server": gateway_url });
    if args.insecure_skip_tls_verify {
        cluster["insecure-skip-tls-verify"] = json!(true);
    }
    let mut context = json!({
        "cluster": args.cluster,
        "user": user_name,
    });
    if let Some(namespace) = args.namespace.as_deref() {
        context["namespace"] = json!(namespace);
    }

    json!({
        "apiVersion": "v1",
        "kind": "Config",
        "clusters": [{
            "name": args.cluster,
            "cluster": cluster,
        }],
        "contexts": [{
            "name": context_name,
            "context": context,
        }],
        "current-context": context_name,
        "users": [{
            "name": user_name,
            "user": {
                "exec": {
                    "apiVersion": "client.authentication.k8s.io/v1",
                    "command": "rush",
                    "args": exec_args,
                    "interactiveMode": "Never",
                    "provideClusterInfo": false,
                }
            }
        }],
    })
}

fn exec_credential(token: &str) -> Value {
    json!({
        "apiVersion": "client.authentication.k8s.io/v1",
        "kind": "ExecCredential",
        "status": {
            "token": token,
        }
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;

    use super::*;

    fn cli() -> Cli {
        Cli::parse_from(["rush", "tail"])
    }

    #[test]
    fn kubeconfig_routes_standard_kubectl_through_gateway() {
        let args = KubeconfigArgs {
            cluster: "prod-us-east-1".to_string(),
            gateway_url: None,
            context: None,
            namespace: Some("payments".to_string()),
            insecure_skip_tls_verify: false,
        };

        let mut cli = cli();
        cli.api_key = Some("must-not-appear".to_string());
        let document = kubeconfig(&cli, &args, "https://gateway.example/k8s/prod");

        assert_eq!(
            document["clusters"][0]["cluster"]["server"],
            "https://gateway.example/k8s/prod"
        );
        assert_eq!(document["current-context"], "rush-prod-us-east-1");
        assert_eq!(document["users"][0]["user"]["exec"]["command"], "rush");
        assert_eq!(document["contexts"][0]["context"]["namespace"], "payments");
        assert!(!document.to_string().contains("must-not-appear"));
    }

    #[test]
    fn kubeconfig_preserves_explicit_config_for_exec_credentials() {
        let mut cli = cli();
        cli.config = Some(PathBuf::from("/tmp/rush-test.toml"));
        let args = KubeconfigArgs {
            cluster: "dev".to_string(),
            gateway_url: None,
            context: Some("dev-via-rush".to_string()),
            namespace: None,
            insecure_skip_tls_verify: true,
        };

        let document = kubeconfig(&cli, &args, "https://gateway.example");
        let exec_args = document["users"][0]["user"]["exec"]["args"]
            .as_array()
            .unwrap();

        assert_eq!(exec_args[0], "--config");
        assert_eq!(exec_args[1], "/tmp/rush-test.toml");
        assert_eq!(
            document["clusters"][0]["cluster"]["insecure-skip-tls-verify"],
            true
        );
    }

    #[test]
    fn exec_credential_uses_the_kubernetes_v1_contract() {
        let document = exec_credential("test-token");

        assert_eq!(document["kind"], "ExecCredential");
        assert_eq!(document["apiVersion"], "client.authentication.k8s.io/v1");
        assert_eq!(document["status"]["token"], "test-token");
    }

    #[test]
    fn rejects_invalid_cluster_identifiers() {
        assert!(validate_cluster("").is_err());
        assert!(validate_cluster("prod\ncluster").is_err());
        assert!(validate_cluster(&"x".repeat(257)).is_err());
        assert!(validate_cluster("prod-us-east-1").is_ok());
    }
}
