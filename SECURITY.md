# Security Policy

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues,
pull requests, or discussions.**

Report privately through GitHub Security Advisories:

1. Go to the [Security tab](https://github.com/RushObservability/cli/security)
2. Click **Report a vulnerability**

This opens a private channel visible only to the maintainers.

### What to include

- The `rush` version (`rush --version`) and your OS and shell
- The command you ran, with any API key redacted
- Steps to reproduce, and what an attacker gains

### What to expect

- **Acknowledgement** within 5 business days.
- **Initial assessment**, including severity, within 10 business days.
- **Progress updates** at least every 10 business days until resolution.
- **Credit** in the advisory and release notes, unless you prefer anonymity.

We ask for a reasonable opportunity to ship a fix before public disclosure,
and aim to release fixes for confirmed high and critical issues within 90 days.

## Supported Versions

This project is pre-1.0. Security fixes land on `main` and in the next release;
there are no backports to earlier tags.

## Scope

`rush` is a terminal client that holds a Rush API key and talks to a
query-api instance. Reports touching the following are especially valuable:

- **Credential exposure** — the API key reaching the process table, shell
  history, log output, error messages, or a crash dump. `--api-key` is
  accepted but `RUSH_API_KEY` is preferred precisely to keep it out of shell
  history; a path that defeats that is in scope.
- **Config file handling** — reading credentials from a world-readable
  location, or writing them with permissive modes.
- **Transport** — any path that would send a key over plaintext HTTP, or
  accept a certificate it should not.
- **Terminal escape injection** — server-controlled log content is rendered
  in a TUI. Content that can emit escape sequences to move the cursor,
  rewrite the screen, or drive a terminal's clipboard or reporting features
  is a genuine injection vector.
- **Output integrity** — anything letting record content forge the framing of
  the newline-delimited JSON stream that downstream tools parse.

**Out of scope:** vulnerabilities in the query-api server itself (report those
against that project), issues that need an already-compromised local account,
and scanner output without a demonstrated impact.

## Our Automated Security Practices

- Formatting, Clippy (`-D warnings`), and the test suite gate every pull
  request, across every platform the release ships
- `cargo audit` for RustSec advisories and `cargo deny` for the dependency,
  license and source policy
- gitleaks scans the full repository history for secrets
- Dependabot version updates for Cargo and GitHub Actions, plus Dependabot
  alerts from the GitHub Advisory Database
- All GitHub Actions are pinned to full commit SHAs
- Workflow tokens are least-privilege (`contents: read`)
