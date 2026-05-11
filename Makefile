SHELL := /bin/zsh

ROOT := /Users/gdc/deadreckon
BIN := $(ROOT)/target/release/deadreckon
DEADRECKON_HOME ?= $(ROOT)/.deadreckon-smoke
STRESS_SECONDS ?= 600

.PHONY: help build release test clippy fmt fmt-check verify smoke doctor alias-zsh stress clean-runtime clean-target

help:
	@printf '%s\n' \
		'deadreckon make targets:' \
		'  make build          Build release binary' \
		'  make verify         build --release, test, clippy -D warnings, fmt --check' \
		'  make smoke          Keyless smoke run using DEADRECKON_HOME under the repo' \
		'  make doctor         Run deadreckon doctor' \
		'  make alias-zsh      Add/update ~/.zshrc alias for deadreckon' \
		'  make stress         Run gated 5-concurrent-run stress test' \
		'  make clean-runtime  Remove repo-local smoke runtime state'

build release:
	cd $(ROOT) && cargo build --release

test:
	cd $(ROOT) && cargo test --workspace

clippy:
	cd $(ROOT) && cargo clippy --workspace -- -D warnings

fmt:
	cd $(ROOT) && cargo fmt

fmt-check:
	cd $(ROOT) && cargo fmt --check

verify: build test clippy fmt-check

smoke: build
	rm -rf $(DEADRECKON_HOME)
	DEADRECKON_HOME=$(DEADRECKON_HOME) $(BIN) run "tiny hello rust" --smoke --sandbox none --max-spend 1
	DEADRECKON_HOME=$(DEADRECKON_HOME) $(BIN) list

doctor: build
	$(BIN) doctor

alias-zsh: build
	@tmp=$$(mktemp); awk '!/^# deadreckon CLI alias$$/ && !/^alias deadreckon=/' /Users/gdc/.zshrc > $$tmp; printf '\n# deadreckon CLI alias\nalias deadreckon='\''/Users/gdc/deadreckon/target/release/deadreckon'\''\n' >> $$tmp; mv $$tmp /Users/gdc/.zshrc
	@printf '%s\n' 'alias installed: open a new shell or run `source /Users/gdc/.zshrc`'

stress:
	cd $(ROOT) && DEADRECKON_STRESS=1 DEADRECKON_STRESS_SECONDS=$(STRESS_SECONDS) cargo test -p deadreckon --test agentic_loop stress_5_concurrent_10min -- --nocapture

clean-runtime:
	rm -rf $(ROOT)/.deadreckon-smoke $(ROOT)/.try-deadreckon

clean-target:
	cd $(ROOT) && cargo clean
