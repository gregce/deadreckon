# bash, not zsh: recipes are POSIX-compatible and CI runners ship no zsh.
SHELL := /bin/bash

ROOT := $(CURDIR)
BIN := $(ROOT)/target/release/deadreckon
DEADRECKON_HOME ?= $(ROOT)/.deadreckon-smoke
STRESS_SECONDS ?= 600
VERIFY_LOG_DIR ?= $(ROOT)/target/verify-timings

.PHONY: help build release test test-agentic test-chain test-codebase test-lifecycle test-smoke-invariant clippy fmt fmt-check public-surface hygiene-recursive verify verify-timed smoke doctor alias-zsh completion-install stress clean-runtime clean-target

help:
	@printf '%s\n' \
		'deadreckon make targets:' \
		'  make build          Build release binary' \
		'  make verify         fmt --check, clippy -D warnings, public surface, test, build' \
		'  make verify-timed   Run verify phases with per-phase timing logs' \
		'  make public-surface Check exported library surface against baseline' \
		'  make test-agentic   Run agentic_loop integration tests' \
		'  make test-chain     Run chain integration tests' \
		'  make test-codebase  Run codebase integration tests' \
		'  make test-lifecycle Run lifecycle integration tests' \
		'  make smoke          Keyless smoke run using DEADRECKON_HOME under the repo' \
		'  make doctor         Run deadreckon doctor' \
		'  make alias-zsh      Add/update ~/.zshrc alias for deadreckon' \
		'  make completion-install  Install shell tab completion' \
		'  make stress         Run gated 5-concurrent-run stress test' \
		'  make clean-runtime  Remove repo-local smoke runtime state'

build release:
	cd $(ROOT) && cargo build --release

test:
	cd $(ROOT) && cargo test --workspace
	cd $(ROOT) && cargo test -p deadreckon --features internal-characterization --test chain --test orchestrate

test-agentic:
	cd $(ROOT) && cargo test -p deadreckon --test agentic_loop

test-chain:
	cd $(ROOT) && cargo test -p deadreckon --features internal-characterization --test chain

test-codebase:
	cd $(ROOT) && cargo test -p deadreckon --test codebase

test-lifecycle:
	cd $(ROOT) && cargo test -p deadreckon --test lifecycle

test-smoke-invariant:
	cd $(ROOT) && cargo test -p deadreckon --test smoke_invariant

clippy:
	cd $(ROOT) && cargo clippy --workspace -- -D warnings

fmt:
	cd $(ROOT) && cargo fmt

fmt-check:
	cd $(ROOT) && cargo fmt --check

public-surface:
	cd $(ROOT) && cargo test -p deadreckon --test public_surface

hygiene-recursive:
	cd $(ROOT) && DEADRECKON_RECURSIVE_VERIFY=1 cargo test -p deadreckon --test hygiene_config

verify:
	$(MAKE) fmt-check
	$(MAKE) clippy
	$(MAKE) public-surface
	$(MAKE) test
	$(MAKE) build

verify-timed:
	mkdir -p $(VERIFY_LOG_DIR)
	set -o pipefail; mkdir -p $(VERIFY_LOG_DIR); /usr/bin/time -p $(MAKE) fmt-check 2>&1 | tee $(VERIFY_LOG_DIR)/01-fmt-check.log
	set -o pipefail; mkdir -p $(VERIFY_LOG_DIR); /usr/bin/time -p $(MAKE) clippy 2>&1 | tee $(VERIFY_LOG_DIR)/02-clippy.log
	set -o pipefail; mkdir -p $(VERIFY_LOG_DIR); /usr/bin/time -p $(MAKE) public-surface 2>&1 | tee $(VERIFY_LOG_DIR)/03-public-surface.log
	set -o pipefail; mkdir -p $(VERIFY_LOG_DIR); /usr/bin/time -p $(MAKE) test 2>&1 | tee $(VERIFY_LOG_DIR)/04-test.log
	set -o pipefail; mkdir -p $(VERIFY_LOG_DIR); /usr/bin/time -p $(MAKE) build 2>&1 | tee $(VERIFY_LOG_DIR)/05-build.log

smoke: build
	rm -rf $(DEADRECKON_HOME)
	DEADRECKON_HOME=$(DEADRECKON_HOME) $(BIN) run "tiny hello rust" --smoke --sandbox none --max-spend 1 --yes --fresh
	DEADRECKON_HOME=$(DEADRECKON_HOME) $(BIN) list

doctor: build
	$(BIN) doctor

alias-zsh: build
	@tmp=$$(mktemp); awk '!/^# deadreckon CLI alias$$/ && !/^alias deadreckon=/' $(HOME)/.zshrc > $$tmp; printf '\n# deadreckon CLI alias\nalias deadreckon='\''$(BIN)'\''\n' >> $$tmp; mv $$tmp $(HOME)/.zshrc
	@printf '%s\n' 'alias installed: open a new shell or run `source ~/.zshrc`'

completion-install: build
	$(BIN) completion install

stress:
	cd $(ROOT) && DEADRECKON_STRESS=1 DEADRECKON_STRESS_SECONDS=$(STRESS_SECONDS) cargo test -p deadreckon --test agentic_loop stress_5_concurrent_10min -- --nocapture

clean-runtime:
	rm -rf $(ROOT)/.deadreckon-smoke $(ROOT)/.try-deadreckon

clean-target:
	cd $(ROOT) && cargo clean
