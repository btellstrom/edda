# The servers are built into a container image (see Dockerfile) and run
# with compose; the client, edda, is built natively.
#
#   make server client    build both
#   make up               start the servers
#   make up HTTPS=1 EDDA_HOST=dnd.example.com
#                         ...behind HTTPS, for players' devices on the network
#   make install          put edda on your PATH
#   make backup           back up the servers' data
#
# Docker instead of podman: make CONTAINER=docker ...

CONTAINER ?= podman
COMPOSE   ?= $(CONTAINER) compose
IMAGE     ?= localhost/edda
# The share of lines the tests must cover (CI fails below it). Raise it as
# coverage rises; never lower it.
COVERAGE_FLOOR = 78
CARGO     ?= cargo
# HTTPS=1: the proxy too (compose's `https` profile).
PROFILES   = $(if $(HTTPS),--profile https)

.DEFAULT_GOAL := help
.PHONY: help all server client install srd up down logs campaign delete-campaign backup restore migrate-sqlite test check coverage coverage-floor deny smoke doc openapi clean

help: ## list the targets
	@awk 'BEGIN { FS = ":.*## " } /^[a-z-]+:.*## / { printf "  make %-10s %s\n", $$1, $$2 }' $(MAKEFILE_LIST)

all: server client ## build the server image and the client

server: ## build the servers' container image
	$(CONTAINER) build --network host -t $(IMAGE) .

client: ## build edda natively (target/release/edda)
	$(CARGO) build --release --locked -p edda

install: ## install edda into ~/.cargo/bin
	$(CARGO) install --locked --path crates/edda
	@echo 'Tab completion (bash): add  source <(COMPLETE=bash edda)  to ~/.bashrc'

srd: ## download the SRD monsters, spells and conditions into data/ (loaded by make up)
	tools/fetch-srd.sh

up: server ## build and start the servers (HTTPS=1 EDDA_HOST=<name>: behind HTTPS too)
	IMAGE=$(IMAGE) $(COMPOSE) $(PROFILES) up -d

down: ## stop the servers (data stays in their volumes)
	$(COMPOSE) --profile https down

logs: ## follow the servers' logs
	$(COMPOSE) logs -f

campaign: ## create a campaign: make campaign NAME="Vox Machina" EDITION=2024
	@test -n "$(NAME)" -a -n "$(EDITION)" || { echo 'usage: make campaign NAME="<campaign name>" EDITION=<2014|2024>' >&2; exit 2; }
	$(COMPOSE) exec auth auth-service new-campaign "$(NAME)" --edition $(EDITION)

delete-campaign: ## delete a campaign and everything in it: make delete-campaign ID=<campaign id>
	@test -n "$(ID)" || { echo 'usage: make delete-campaign ID=<campaign id>  (edda auth status shows it)' >&2; exit 2; }
	@if [ -z "$(YES)" ]; then \
		printf 'Delete campaign %s with all its characters, combats, plans, monsters and devices? This cannot be undone. [y/N] ' "$(ID)"; \
		read answer; [ "$$answer" = y ] || [ "$$answer" = Y ] || { echo 'Not deleted.'; exit 1; }; \
	fi
	$(COMPOSE) exec auth auth-service delete-campaign $(ID)
	$(COMPOSE) exec characters character-service delete-campaign $(ID)
	$(COMPOSE) exec combat combat-service delete-campaign $(ID)
	$(COMPOSE) exec monsters monster-service delete-campaign $(ID)

backup: ## back up the servers' data into backups/<date> (they keep running)
	CONTAINER=$(CONTAINER) tools/backup.sh

restore: ## put a backup back: make restore FROM=backups/<date> (replaces all data)
	@test -n "$(FROM)" || { echo 'usage: make restore FROM=backups/<date>' >&2; exit 2; }
	CONTAINER=$(CONTAINER) tools/restore.sh "$(FROM)"

migrate-sqlite: ## once, after upgrading: bring characters and monsters over from SQLite
	$(COMPOSE) run --rm --no-deps characters character-service import-sqlite /data/characters.db
	$(COMPOSE) run --rm --no-deps monsters monster-service import-sqlite /data/monsters.db
	@# The shared library is the old one now; the SRD again over it, by name.
	$(COMPOSE) run --rm seed-monsters

test: ## run all tests, against a throwaway Postgres of their own
	CONTAINER=$(CONTAINER) tools/with-test-postgres.sh $(CARGO) test --workspace --locked

check: ## what CI checks: formatting, lints (warnings fail), the docs, the tests and the guard hook
	python3 -I .claude/hooks/guard.py --self-test
	$(CARGO) fmt --all --check
	$(CARGO) clippy --workspace --all-targets --locked -- -D warnings
	RUSTDOCFLAGS="-D warnings" $(CARGO) doc --workspace --no-deps --locked
	CONTAINER=$(CONTAINER) tools/with-test-postgres.sh $(CARGO) test --workspace --locked

coverage: ## what the tests cover, failing below COVERAGE_FLOOR (cargo install cargo-llvm-cov)
	CONTAINER=$(CONTAINER) tools/with-test-postgres.sh $(CARGO) llvm-cov --workspace --locked \
		--summary-only --fail-under-lines $(COVERAGE_FLOOR) -- --include-ignored

coverage-floor:
	@echo $(COVERAGE_FLOOR)

deny: ## the dependencies' licences, advisories and sources (cargo install cargo-deny)
	cargo deny check

smoke: client ## end to end: the compose stack, seeded, driven by edda (needs make srd)
	CONTAINER=$(CONTAINER) tools/smoke-test.sh

doc: ## the Rust docs, opened in a browser (target/doc)
	$(CARGO) doc --workspace --no-deps --locked --open

openapi: ## write docs/openapi.json anew, after changing a route
	UPDATE_OPENAPI=1 $(CARGO) test --locked -p edda-client --test openapi

clean: ## remove build output and the server image
	$(CARGO) clean
	-$(CONTAINER) rmi $(IMAGE)
