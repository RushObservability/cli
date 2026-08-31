//! Tests for the generated completion scripts and man pages.
//!
//! Uses std::process only, so it needs no dev-dependency and does not collide
//! with the other integration test files.

use std::process::Command;

fn rush(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_rush"))
        .args(args)
        .output()
        .expect("the rush binary should be built for tests");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn completions_are_generated_for_every_supported_shell() {
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let (stdout, stderr, code) = rush(&["completions", shell]);
        assert_eq!(code, 0, "{shell} exited {code}: {stderr}");
        assert!(!stdout.is_empty(), "{shell} produced no script");
        // A completion script that does not know the subcommand is useless.
        assert!(
            stdout.contains("tail"),
            "{shell} script omits the tail subcommand"
        );
    }
}

#[test]
fn completions_reject_an_unknown_shell() {
    let (_, _, code) = rush(&["completions", "notashell"]);
    assert_eq!(code, 2, "an unknown shell should fail argument parsing");
}

#[test]
fn top_level_man_page_is_roff_and_lists_global_options() {
    let (stdout, _, code) = rush(&["man"]);
    assert_eq!(code, 0);
    assert!(stdout.starts_with(".ie"), "should be roff");
    assert!(stdout.contains(".TH rush 1"), "missing man title header");
    // Hyphens are roff-escaped, hence the backslashes.
    for opt in ["\\-\\-url", "\\-\\-tenant", "\\-\\-api\\-key"] {
        assert!(stdout.contains(opt), "man page omits {opt}");
    }
}

#[test]
fn subcommand_man_page_documents_that_subcommands_flags() {
    let (stdout, _, code) = rush(&["man", "tail"]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains(".TH rush-tail 1"),
        "should be titled rush-tail"
    );
    for opt in ["\\-\\-search", "\\-\\-filter", "\\-\\-output"] {
        assert!(stdout.contains(opt), "rush-tail page omits {opt}");
    }
}

#[test]
fn man_rejects_an_unknown_subcommand() {
    let (_, stderr, code) = rush(&["man", "nosuchcommand"]);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("nosuchcommand"),
        "error should name the bad subcommand"
    );
}
