# Heavily inspired by Lighthouse: https://github.com/sigp/lighthouse/blob/stable/Makefile
# and Reth: https://github.com/paradigmxyz/reth/blob/main/Makefile
.DEFAULT_GOAL := help

GIT_VER ?= $(shell git describe --tags --always --dirty="-dev")
GIT_TAG ?= $(shell git describe --tags --abbrev=0)

FEATURES ?=

##@ Help

.PHONY: help
help: ## Display this help.
	@awk 'BEGIN {FS = ":.*##"; printf "Usage:\n  make \033[36m<target>\033[0m\n"} /^[a-zA-Z_0-9-]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)

.PHONY: v
v: ## Show the current version
	@echo "Version: ${GIT_VER}"

##@ Build

.PHONY: clean
clean: ## Clean up
	cargo clean

.PHONY: build
build: ## Build (debug version)
	cargo build --features "$(FEATURES)"

.PHONY: docker-image-rbuilder
docker-image-rubilder: ## Build a rbuilder Docker image
	docker build --platform linux/amd64 --target rbuilder-runtime --build-arg FEATURES="$(FEATURES)"  . -t rbuilder

.PHONY: docker-image-test-relay
docker-image-test-relay: ## Build a test relay Docker image
	docker build --platform linux/amd64 --target test-relay-runtime --build-arg FEATURES="$(FEATURES)" . -t test-relay

##@ Dev

.PHONY: lint
lint: ## Run the linters
	cargo fmt -- --check
	cargo clippy --features "$(FEATURES)" -- -D warnings
	cargo clippy -p op-rbuilder --features "$(FEATURES),optimism" -- -D warnings

.PHONY: test
test: ## Run the tests for rbuilder and op-rbuilder
	cargo test --verbose --features "$(FEATURES)"
	cargo test -p op-rbuilder --verbose --features "$(FEATURES),optimism"

.PHONY: lt
lt: lint test ## Run "lint" and "test"

.PHONY: fmt
fmt: ## Format the code
	cargo fmt
	cargo fix --allow-staged
	cargo clippy --features "$(FEATURES)" --fix --allow-staged
	cargo clippy -p op-rbuilder --features "$(FEATURES),optimism" --fix --allow-staged

.PHONY: bench
bench: ## Run benchmarks
	cargo bench --features "$(FEATURES)" --workspace

.PHONY: bench-report-open
bench-report-open: ## Open last benchmark report in the browser
	open "target/criterion/report/index.html"

.PHONY: bench-in-ci
bench-in-ci: ## Run benchmarks in CI (adds timestamp and version to the report, customizes Criterion output)
	./scripts/ci/benchmark-in-ci.sh

.PHONY: bench-clean
bench-clean: ## Remove previous benchmark data
	rm -rf target/criterion
	rm -rf target/benchmark-in-ci
	rm -rf target/benchmark-html-dev

.PHONY: bench-prettify
bench-prettify: ## Prettifies the latest Criterion report
	rm -rf target/benchmark-html-dev
	./scripts/ci/criterion-prettify-report.sh target/criterion target/benchmark-html-dev
	@echo "\nopen target/benchmark-html-dev/report/index.html"

.PHONY: validate-config
validate-config: ## Validate the correctness of the configuration files
	@for CONFIG in $(shell ls config-*.toml); do \
		cargo run --bin validate-config -- --config $$CONFIG; \
	done
