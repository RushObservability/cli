use super::*;
use clap::Parser;

fn load(contents: &str, flags: &[&str], vars: &[(&str, &str)]) -> Config {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, contents).unwrap();
    let mut args = vec!["rush", "--config", path.to_str().unwrap(), "tail"];
    args.extend(flags);
    let cli = Cli::parse_from(args);
    let crate::cli::Command::Tail(tail) = &cli.command else {
        unreachable!()
    };
    Config::load_with_env(&cli, tail, |key| {
        vars.iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.to_string())
            .filter(|v| !v.trim().is_empty())
    })
    .unwrap()
}

#[test]
fn configuration_precedence_covers_every_option() {
    let file = r#"
url = "https://file.example/"
web_url = "https://file-web.example/"
tenant = "file"
api_key = "file-key"
poll_interval_ms = 2000
window_seconds = 200
buffer_size = 200
"#;
    let vars = [
        ("RUSH_URL", "https://env.example/"),
        ("RUSH_WEB_URL", "https://env-web.example/"),
        ("RUSH_TENANT", "env"),
        ("RUSH_API_KEY", "env-key"),
        ("RUSH_POLL_INTERVAL_MS", "3000"),
        ("RUSH_WINDOW_SECONDS", "300"),
        ("RUSH_BUFFER_SIZE", "300"),
    ];
    let flags = [
        "--url",
        "https://flag.example/",
        "--web-url",
        "https://flag-web.example/",
        "--tenant",
        "flag",
        "--api-key",
        "flag-key",
        "--poll-interval-ms",
        "4000",
        "--window-seconds",
        "400",
        "--buffer-size",
        "400",
    ];
    for (flags, vars, source, number) in [
        (&[][..], &[][..], "file", 200),
        (&[][..], &vars[..], "env", 300),
        (&flags[..], &vars[..], "flag", 400),
    ] {
        let config = load(file, flags, vars);
        assert_eq!(config.url, format!("https://{source}.example"));
        assert_eq!(config.web_url, format!("https://{source}-web.example"));
        assert_eq!(config.tenant, source);
        assert_eq!(config.api_key, Some(format!("{source}-key")));
        assert_eq!(config.poll_interval_ms, number * 10);
        assert_eq!(config.window_seconds, number);
        assert_eq!(config.buffer_size, number as usize);
    }
}

#[test]
fn defaults_and_blank_environment_values() {
    let config = load(
        "",
        &[],
        &[
            ("RUSH_URL", " "),
            ("RUSH_API_KEY", ""),
            ("RUSH_TENANT", "\t"),
        ],
    );
    assert_eq!(config.url, "http://localhost:8080");
    assert_eq!(config.web_url, "http://localhost:5173");
    assert_eq!(config.tenant, "default");
    assert!(config.api_key.is_none());
    assert_eq!(
        (
            config.poll_interval_ms,
            config.window_seconds,
            config.buffer_size
        ),
        (1000, 300, 5000)
    );
}

#[test]
fn numeric_bounds_and_invalid_environment_values() {
    for (value, expected) in [
        ("0", (250, 10, 100)),
        ("18446744073709551615", (60000, 604800, 100000)),
    ] {
        let config = load(
            "",
            &[],
            &[
                ("RUSH_POLL_INTERVAL_MS", value),
                ("RUSH_WINDOW_SECONDS", value),
                ("RUSH_BUFFER_SIZE", value),
            ],
        );
        assert_eq!(
            (
                config.poll_interval_ms,
                config.window_seconds,
                config.buffer_size
            ),
            expected
        );
    }
    for invalid in ["-1", "nonsense", "184467440737095516160"] {
        let config = load(
            "poll_interval_ms=900\nwindow_seconds=90\nbuffer_size=900",
            &[],
            &[
                ("RUSH_POLL_INTERVAL_MS", invalid),
                ("RUSH_WINDOW_SECONDS", invalid),
                ("RUSH_BUFFER_SIZE", invalid),
            ],
        );
        assert_eq!(
            (
                config.poll_interval_ms,
                config.window_seconds,
                config.buffer_size
            ),
            (900, 90, 900)
        );
    }
}

#[test]
fn explicitly_missing_config_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    let cli = Cli::parse_from(["rush", "--config", missing.to_str().unwrap(), "tail"]);
    let crate::cli::Command::Tail(tail) = &cli.command else {
        unreachable!()
    };
    assert!(
        Config::load_with_env(&cli, tail, |_| None)
            .unwrap_err()
            .to_string()
            .contains("does not exist")
    );
}

#[test]
fn invalid_urls_are_rejected() {
    for url in [
        "",
        "relative/path",
        "file:///tmp/api",
        "ftp://api.example",
        "https://",
    ] {
        assert!(validate_base_url(url, "API").is_err(), "{url}");
    }
}
