use std::{
    collections::HashMap,
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveDateTime, Utc};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    cli::{Cli, CredentialArgs, KubeconfigArgs, KubernetesArgs, KubernetesCommand},
    config,
};

const CREDENTIAL_REFRESH_SKEW_SECONDS: i64 = 30;

#[derive(Debug, Deserialize)]
struct LoginStartResponse {
    device_code: String,
    user_code: String,
    expires_in: i64,
    interval: u64,
}

#[derive(Debug, Deserialize)]
struct LoginTokenResponse {
    status: String,
    access_token: Option<String>,
    expires_at: Option<String>,
    interval: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CachedCredential {
    token: String,
    expires_at: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CredentialCache {
    credentials: HashMap<String, CachedCredential>,
}

pub async fn run(cli: &Cli, args: &KubernetesArgs) -> Result<()> {
    match &args.command {
        KubernetesCommand::Kubeconfig(args) => print_kubeconfig(cli, args),
        KubernetesCommand::Credential(args) => print_credential(cli, args).await,
    }
}

fn print_kubeconfig(cli: &Cli, args: &KubeconfigArgs) -> Result<()> {
    validate_cluster(&args.cluster)?;
    let gateway_url = config::kubernetes_gateway_url(cli, args.gateway_url.clone())?;
    let document = kubeconfig(cli, args, &gateway_url);
    write_json(&document)
}

async fn print_credential(cli: &Cli, args: &CredentialArgs) -> Result<()> {
    validate_cluster(&args.cluster)?;
    let (api_url, web_url) = config::kubernetes_login_urls(cli)?;
    let cache_key = format!("{api_url}|{}", args.cluster);
    let cache_path = credential_cache_path()?;
    let mut cache = read_credential_cache(&cache_path)?;
    let credential = match cache
        .credentials
        .get(&cache_key)
        .filter(|credential| credential_is_fresh(credential))
        .cloned()
    {
        Some(credential) => credential,
        None => {
            let credential = browser_login(
                &api_url,
                &web_url,
                &args.cluster,
                env::var_os("RUSH_KUBERNETES_NO_BROWSER").is_none(),
            )
            .await?;
            cache.credentials.insert(cache_key, credential.clone());
            write_credential_cache(&cache_path, &cache)?;
            credential
        }
    };
    write_json(&exec_credential(&credential.token, &credential.expires_at))
}

async fn browser_login(
    api_url: &str,
    web_url: &str,
    cluster: &str,
    open_browser: bool,
) -> Result<CachedCredential> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .context("failed to create Kubernetes login client")?;
    let start_response = client
        .post(format!("{api_url}/api/v1/kubernetes/login/start"))
        .json(&json!({ "cluster_id": cluster }))
        .send()
        .await
        .context("failed to start Rush Kubernetes login")?;
    if !start_response.status().is_success() {
        let status = start_response.status();
        let message = start_response.text().await.unwrap_or_default();
        bail!(
            "Rush Kubernetes login could not start ({status}): {}",
            message.trim()
        )
    }
    let started = start_response
        .json::<LoginStartResponse>()
        .await
        .context("Rush returned an invalid Kubernetes login response")?;
    let login_url = format!("{web_url}/kubernetes-access/login/{}", started.user_code);
    eprintln!("Approve kubectl access in your browser:\n{login_url}");
    if open_browser {
        if let Err(error) = open::that(&login_url) {
            eprintln!("Could not open the browser automatically: {error}");
        }
    }

    let deadline = Instant::now() + Duration::from_secs(started.expires_in.max(1) as u64);
    let mut interval = started.interval.clamp(1, 10);
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        let response = client
            .post(format!("{api_url}/api/v1/kubernetes/login/token"))
            .json(&json!({ "device_code": started.device_code }))
            .send()
            .await
            .context("failed while waiting for Rush Kubernetes login")?;
        if response.status() == reqwest::StatusCode::ACCEPTED {
            if let Ok(pending) = response.json::<LoginTokenResponse>().await {
                interval = pending.interval.clamp(1, 10);
            }
            continue;
        }
        if !response.status().is_success() {
            let status = response.status();
            let message = response.text().await.unwrap_or_default();
            bail!(
                "Rush Kubernetes login failed ({status}): {}",
                message.trim()
            )
        }
        let approved = response
            .json::<LoginTokenResponse>()
            .await
            .context("Rush returned an invalid temporary credential")?;
        if approved.status != "approved" {
            bail!("Rush returned an unexpected Kubernetes login status")
        }
        let token = approved
            .access_token
            .filter(|token| token.starts_with("rkt1_"))
            .context("Rush did not return a temporary Kubernetes credential")?;
        let expires_at = approved
            .expires_at
            .context("Rush did not return the Kubernetes credential expiry")?;
        parse_expiry(&expires_at)
            .context("Rush returned an invalid Kubernetes credential expiry")?;
        return Ok(CachedCredential { token, expires_at });
    }
    bail!("Rush Kubernetes login expired before it was approved")
}

fn credential_cache_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("RUSH_KUBERNETES_CREDENTIAL_CACHE") {
        return Ok(PathBuf::from(path));
    }
    ProjectDirs::from("com", "RushObservability", "rush")
        .map(|dirs| dirs.config_dir().join("kubernetes-credentials.json"))
        .context("could not determine the Rush configuration directory")
}

fn read_credential_cache(path: &Path) -> Result<CredentialCache> {
    match fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents)
            .with_context(|| format!("invalid credential cache {}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(CredentialCache::default()),
        Err(error) => Err(error)
            .with_context(|| format!("failed to read credential cache {}", path.display())),
    }
}

fn write_credential_cache(path: &Path, cache: &CredentialCache) -> Result<()> {
    let parent = path
        .parent()
        .context("credential cache path has no parent")?;
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create credential cache directory {}",
            parent.display()
        )
    })?;
    set_directory_permissions(parent)?;
    let temporary = path.with_extension("json.tmp");
    let encoded = serde_json::to_vec_pretty(cache)?;
    write_private_file(&temporary, &encoded)?;
    fs::rename(&temporary, path)
        .with_context(|| format!("failed to replace credential cache {}", path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn set_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn write_private_file(path: &Path, contents: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, contents: &[u8]) -> Result<()> {
    fs::write(path, contents)?;
    Ok(())
}

fn parse_expiry(value: &str) -> Result<DateTime<Utc>> {
    if let Ok(value) = DateTime::parse_from_rfc3339(value) {
        return Ok(value.with_timezone(&Utc));
    }
    let value = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")?;
    Ok(value.and_utc())
}

fn credential_is_fresh(credential: &CachedCredential) -> bool {
    credential.token.starts_with("rkt1_")
        && parse_expiry(&credential.expires_at).is_ok_and(|expires_at| {
            expires_at > Utc::now() + chrono::Duration::seconds(CREDENTIAL_REFRESH_SKEW_SECONDS)
        })
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

fn exec_credential(token: &str, expires_at: &str) -> Value {
    let expiration_timestamp = parse_expiry(expires_at)
        .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|_| expires_at.to_string());
    json!({
        "apiVersion": "client.authentication.k8s.io/v1",
        "kind": "ExecCredential",
        "status": {
            "token": token,
            "expirationTimestamp": expiration_timestamp,
        }
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;
    use httpmock::{Method::POST, MockServer};

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
        let document = exec_credential("test-token", "2026-08-22 12:30:00");

        assert_eq!(document["kind"], "ExecCredential");
        assert_eq!(document["apiVersion"], "client.authentication.k8s.io/v1");
        assert_eq!(document["status"]["token"], "test-token");
        assert_eq!(
            document["status"]["expirationTimestamp"],
            "2026-08-22T12:30:00Z"
        );
    }

    #[test]
    fn only_temporary_login_credentials_are_reused() {
        let valid = CachedCredential {
            token: format!("rkt1_{}", "a".repeat(64)),
            expires_at: (Utc::now() + chrono::Duration::minutes(5))
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
        };
        assert!(credential_is_fresh(&valid));

        let mut api_key = valid.clone();
        api_key.token = "rush_query_api_key".to_string();
        assert!(!credential_is_fresh(&api_key));
    }

    #[tokio::test]
    async fn browser_login_uses_no_configured_api_key() {
        let server = MockServer::start();
        let leaked_api_key = server.mock(|when, then| {
            when.method(POST)
                .header("authorization", "Bearer configured-api-key");
            then.status(500);
        });
        let device_code = format!("rkt1_{}", "a".repeat(64));
        let start = server.mock(|when, then| {
            when.method(POST)
                .path("/api/v1/kubernetes/login/start")
                .body_contains("\"cluster_id\":\"orbstack\"");
            then.status(200).json_body(json!({
                "device_code": device_code,
                "user_code": "A1B2C3D4E5F60708",
                "expires_in": 30,
                "interval": 1
            }));
        });
        let poll = server.mock(|when, then| {
            when.method(POST)
                .path("/api/v1/kubernetes/login/token")
                .body_contains("rkt1_");
            then.status(200).json_body(json!({
                "status": "approved",
                "access_token": format!("rkt1_{}", "a".repeat(64)),
                "expires_at": "2026-08-22 13:00:00",
                "interval": 1
            }));
        });

        let credential = browser_login(
            &server.base_url(),
            "http://localhost:5173",
            "orbstack",
            false,
        )
        .await
        .unwrap();

        start.assert();
        poll.assert();
        leaked_api_key.assert_hits(0);
        assert!(credential.token.starts_with("rkt1_"));
    }

    #[test]
    fn rejects_invalid_cluster_identifiers() {
        assert!(validate_cluster("").is_err());
        assert!(validate_cluster("prod\ncluster").is_err());
        assert!(validate_cluster(&"x".repeat(257)).is_err());
        assert!(validate_cluster("prod-us-east-1").is_ok());
    }
}
