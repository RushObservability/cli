use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::model::Signal;

#[derive(Debug, Parser)]
#[command(
    name = "rush",
    version,
    about = "Live-tail Rush logs and APM from your terminal"
)]
pub struct Cli {
    /// Optional TOML config. Defaults to ~/.config/rush/config.toml.
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Rush query-api base URL. Overrides RUSH_URL and config.
    #[arg(long, global = true)]
    pub url: Option<String>,

    /// Rush web UI base URL used by the `o` key.
    #[arg(long, global = true)]
    pub web_url: Option<String>,

    /// Tenant name. An API key remains scoped to the tenant that issued it.
    #[arg(long, global = true)]
    pub tenant: Option<String>,

    /// API key. Prefer RUSH_API_KEY so it does not appear in shell history.
    #[arg(long, global = true, hide_env_values = true)]
    pub api_key: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Follow recent telemetry in an interactive TUI or newline-delimited JSON.
    Tail(TailArgs),

    /// Configure standard kubectl access through a Rush gateway.
    Kubernetes(KubernetesArgs),
}

#[derive(Debug, Clone, Args)]
pub struct KubernetesArgs {
    #[command(subcommand)]
    pub command: KubernetesCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum KubernetesCommand {
    /// Print a kubeconfig that routes one cluster through the Rush gateway.
    Kubeconfig(KubeconfigArgs),

    /// Return credentials for a generated kubeconfig.
    #[command(hide = true)]
    Credential(CredentialArgs),
}

#[derive(Debug, Clone, Args)]
pub struct KubeconfigArgs {
    /// Rush cluster identifier.
    #[arg(long)]
    pub cluster: String,

    /// Full Kubernetes API URL exposed by the Rush gateway.
    #[arg(long)]
    pub gateway_url: Option<String>,

    /// Context name written to the kubeconfig.
    #[arg(long)]
    pub context: Option<String>,

    /// Default namespace written to the context.
    #[arg(long)]
    pub namespace: Option<String>,

    /// Skip verification of the gateway TLS certificate.
    #[arg(long)]
    pub insecure_skip_tls_verify: bool,
}

#[derive(Debug, Clone, Args)]
pub struct CredentialArgs {
    /// Rush cluster identifier.
    #[arg(long)]
    pub cluster: String,
}

#[derive(Debug, Clone, Args)]
pub struct TailArgs {
    /// Signal to tail.
    #[arg(value_enum, default_value_t = Signal::Logs)]
    pub signal: Signal,

    /// Server-side free-text search.
    #[arg(short = 'q', long)]
    pub search: Option<String>,

    /// Structured filter, e.g. service_name=gateway or duration_ns>=100000000.
    #[arg(short = 'f', long = "filter")]
    pub filters: Vec<String>,

    /// Sliding query window in seconds.
    #[arg(long)]
    pub window_seconds: Option<u64>,

    /// Poll interval in milliseconds.
    #[arg(long)]
    pub poll_interval_ms: Option<u64>,

    /// Maximum records retained locally.
    #[arg(long)]
    pub buffer_size: Option<usize>,

    /// Maximum rows requested per poll.
    #[arg(long, default_value_t = 500, value_parser = clap::value_parser!(u16).range(1..=1000))]
    pub limit: u16,

    /// Output mode. JSON is useful for pipes and scripts.
    #[arg(long, value_enum, default_value_t = OutputMode::Tui)]
    pub output: OutputMode,
}

#[derive(Debug, Clone, Copy, Default, ValueEnum, PartialEq, Eq)]
pub enum OutputMode {
    #[default]
    Tui,
    Json,
}
