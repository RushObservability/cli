.DEFAULT_GOAL := help

CARGO ?= cargo
ARGS ?=

.PHONY: help fmt fmt-check check lint test build release ci install run-logs run-apm clean

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

fmt: ## Format Rust source
	$(CARGO) fmt --all

fmt-check: ## Verify Rust formatting without changing files
	$(CARGO) fmt --all -- --check

check: ## Type-check every target and feature
	$(CARGO) check --locked --all-targets --all-features

lint: ## Run Clippy and reject warnings
	$(CARGO) clippy --locked --all-targets --all-features -- -D warnings

test: ## Run the test suite
	$(CARGO) test --locked

build: ## Build every target in debug mode
	$(CARGO) build --locked --all-targets

release: ## Build the optimized rush binary
	$(CARGO) build --locked --release

ci: fmt-check lint test build ## Run all pull-request checks locally

install: ## Install rush from this checkout
	$(CARGO) install --locked --path .

run-logs: ## Start the logs TUI; pass options with ARGS="..."
	$(CARGO) run -- tail logs $(ARGS)

run-apm: ## Start the APM TUI; pass options with ARGS="..."
	$(CARGO) run -- tail apm $(ARGS)

clean: ## Remove Cargo build artifacts
	$(CARGO) clean
