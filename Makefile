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
.PHONY: help all server client install up down logs campaign test clean

help: ## list the targets
	@awk 'BEGIN { FS = ":.*## " } /^[a-z-]+:.*## / { printf "  make %-10s %s\n", $$1, $$2 }' $(MAKEFILE_LIST)

all: server client ## build the server image and the client

server: ## build the servers' container image
	$(CONTAINER) build --network host -t $(IMAGE) .

client: ## build campaign-edit natively (target/release/campaign-edit)
	$(CARGO) build --release --locked -p campaign-edit

install: ## install campaign-edit into ~/.cargo/bin
	$(CARGO) install --locked --path crates/campaign-edit

up: server ## build and start the servers
	$(COMPOSE) up -d

down: ## stop the servers (data stays in their volumes)
	$(COMPOSE) down

logs: ## follow the servers' logs
	$(COMPOSE) logs -f

campaign: ## create a campaign: make campaign NAME="Vox Machina"
	@test -n "$(NAME)" || { echo 'usage: make campaign NAME="<campaign name>"' >&2; exit 2; }
	$(COMPOSE) exec auth auth-service new-campaign "$(NAME)"

test: ## run all tests (needs Postgres: make up, or compose up -d postgres)
	$(CARGO) test --workspace

clean: ## remove build output and the server image
	$(CARGO) clean
	-$(CONTAINER) rmi $(IMAGE)
