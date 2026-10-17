# The servers are built into a container image (see Dockerfile) and run
# with compose; the client, campaign-edit, is built natively.
#
#   make server client    build both
#   make up               start the servers
#   make install          put campaign-edit on your PATH
#
# Docker instead of podman: make CONTAINER=docker ...

CONTAINER ?= podman
COMPOSE   ?= $(CONTAINER) compose
IMAGE     ?= localhost/campaign-manager
CARGO     ?= cargo

.DEFAULT_GOAL := help
.PHONY: help all server client install srd up down logs campaign delete-campaign test clean

help: ## list the targets
	@awk 'BEGIN { FS = ":.*## " } /^[a-z-]+:.*## / { printf "  make %-10s %s\n", $$1, $$2 }' $(MAKEFILE_LIST)

all: server client ## build the server image and the client

server: ## build the servers' container image
	$(CONTAINER) build --network host -t $(IMAGE) .

client: ## build campaign-edit natively (target/release/campaign-edit)
	$(CARGO) build --release --locked -p campaign-edit

install: ## install campaign-edit into ~/.cargo/bin
	$(CARGO) install --locked --path crates/campaign-edit
	@echo 'Tab completion (bash): add  source <(COMPLETE=bash campaign-edit)  to ~/.bashrc'

srd: ## download the SRD monsters, spells and conditions into data/ (loaded by make up)
	tools/fetch-srd.sh

up: server ## build and start the servers
	$(COMPOSE) up -d

down: ## stop the servers (data stays in their volumes)
	$(COMPOSE) down

logs: ## follow the servers' logs
	$(COMPOSE) logs -f

campaign: ## create a campaign: make campaign NAME="Vox Machina" EDITION=2024
	@test -n "$(NAME)" -a -n "$(EDITION)" || { echo 'usage: make campaign NAME="<campaign name>" EDITION=<2014|2024>' >&2; exit 2; }
	$(COMPOSE) exec auth auth-service new-campaign "$(NAME)" --edition $(EDITION)

delete-campaign: ## delete a campaign and everything in it: make delete-campaign ID=<campaign id>
	@test -n "$(ID)" || { echo 'usage: make delete-campaign ID=<campaign id>  (campaign-edit auth status shows it)' >&2; exit 2; }
	@if [ -z "$(YES)" ]; then \
		printf 'Delete campaign %s with all its characters, combats, plans, monsters and devices? This cannot be undone. [y/N] ' "$(ID)"; \
		read answer; [ "$$answer" = y ] || [ "$$answer" = Y ] || { echo 'Not deleted.'; exit 1; }; \
	fi
	$(COMPOSE) exec auth auth-service delete-campaign $(ID)
	$(COMPOSE) exec characters character-service delete-campaign $(ID)
	$(COMPOSE) exec combat combat-service delete-campaign $(ID)
	$(COMPOSE) exec monsters monster-service delete-campaign $(ID)

test: ## run all tests (needs Postgres: make up, or compose up -d postgres)
	$(CARGO) test --workspace

clean: ## remove build output and the server image
	$(CARGO) clean
	-$(CONTAINER) rmi $(IMAGE)
