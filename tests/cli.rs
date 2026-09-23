//! End-to-end tests that invoke the built `rush` binary.
//!
//! The unit tests cover internal behaviour; these cover the CLI *contract*:
//! argument parsing, exit codes, and help output. That contract is what users
//! and scripts depend on, and none of it is exercised by unit tests.
//!
//! Everything here is offline -- no test reaches the network or needs config.

use assert_cmd::Command;
use predicates::str::contains;

fn rush() -> Command {
    Command::cargo_bin("rush").expect("the `rush` binary should be built for tests")
}

#[test]
fn no_arguments_prints_usage_and_fails() {
    // A bare invocation must not look like success to a shell script.
    rush()
        .assert()
        .failure()
        .code(2)
        .stderr(contains("Usage: rush"));
}

#[test]
fn help_succeeds_and_describes_the_tool() {
    rush()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Live-tail Rush logs and APM"))
        .stdout(contains("tail"));
}

#[test]
fn version_matches_the_crate_version() {
    rush()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn unknown_subcommand_fails() {
    rush()
        .arg("definitely-not-a-command")
        .assert()
        .failure()
        .code(2);
}

#[test]
fn unknown_flag_fails() {
    rush()
        .arg("--definitely-not-a-flag")
        .assert()
        .failure()
        .code(2);
}

#[test]
fn tail_help_documents_its_options() {
    rush()
        .args(["tail", "--help"])
        .assert()
        .success()
        .stdout(contains("--search"))
        .stdout(contains("--filter"))
        .stdout(contains("--window-seconds"));
}

#[test]
fn tail_rejects_an_unknown_signal() {
    // The signal is a ValueEnum; a typo must fail at parse time rather than
    // reaching the API layer.
    rush()
        .args(["tail", "notasignal"])
        .assert()
        .failure()
        .code(2)
        .stderr(contains("notasignal"));
}

#[test]
fn config_errors_do_not_expose_secrets() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    for contents in [
        "api_key = \"review-dummy-secret\" unexpected\n",
        "api_key = \"review-dummy-secret\n",
        "buffer_size = \"review-dummy-secret\"\n",
        "api_key = [\"review-dummy-secret\"]\n",
    ] {
        std::fs::write(&path, contents).unwrap();
        rush()
            .arg("--config")
            .arg(&path)
            .args(["tail", "--output", "json"])
            .assert()
            .failure()
            .stderr(contains("invalid config"))
            .stderr(contains("line 1, column"))
            .stderr(predicates::prelude::PredicateBooleanExt::not(contains(
                "review-dummy-secret",
            )));
    }
}

#[test]
fn insecure_http_environment_values_are_respected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(
        &path,
        "api_key = \"review-dummy-secret\"\nurl = \"http://example.invalid\"\n",
    )
    .unwrap();
    for (value, expected) in [
        ("false", "refusing to send the API key over plaintext HTTP"),
        ("0", "refusing to send the API key over plaintext HTTP"),
        ("true", "TUI output requires a terminal"),
        ("1", "TUI output requires a terminal"),
        ("typo", "RUSH_ALLOW_INSECURE_HTTP must be"),
    ] {
        // Captured stdout makes TUI mode exit before any network request.
        rush()
            .env_remove("RUSH_URL")
            .env_remove("RUSH_API_KEY")
            .env("RUSH_ALLOW_INSECURE_HTTP", value)
            .arg("--config")
            .arg(&path)
            .arg("tail")
            .assert()
            .failure()
            .stderr(contains(expected));
    }
}
