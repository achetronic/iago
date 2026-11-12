# Iago Makefile

BIN_NAME  := iago
CARGO     ?= cargo
PREFIX    ?= $(HOME)/.local
BINDIR    := $(PREFIX)/bin
APPDIR    := $(HOME)/.local/share/applications
ICONDIR   := $(HOME)/.local/share/icons/hicolor/scalable/apps
APP_ID    := com.achetronic.Iago

.DEFAULT_GOAL := help

.PHONY: help build release build-ui run daemon status fmt fmt-check vet clippy \
        test check ci clean install uninstall reinstall deps dist clean-dist

help: ## Show this help
	@echo "Iago - local dictation"
	@echo ""
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| sort \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'
	@echo ""
	@echo "VERSION=$(VERSION) SUFFIX=$(SUFFIX)"

################################################################################
## Build
################################################################################

# One binary: the window, the daemon and the CLI are all `iago`. The window runs
# in its own process, spawned by `iago ui`, so a crash there never takes the
# daemon down.
build: ## Debug build of the single binary
	$(CARGO) build -p $(BIN_NAME)

release: ## Optimised build of the single binary
	$(CARGO) build --release --locked $(CARGO_TARGET) -p $(BIN_NAME)

run: ## Open the control center
	$(CARGO) run -p iago

daemon: build ## Run the daemon in the foreground
	$(CARGO) run -- -v daemon run

status: ## Print what a running daemon is doing
	$(CARGO) run -- ctl daemon-status

record: ## Start or stop dictation through a running daemon
	$(CARGO) run -- ctl

################################################################################
## Quality
################################################################################

fmt: ## Format the code
	$(CARGO) fmt --all

fmt-check: ## Fail if the code is not formatted
	$(CARGO) fmt --all --check

vet: ## Type-check every feature combination
	$(CARGO) check --workspace --all-targets
	$(CARGO) check --no-default-features
	$(CARGO) check --no-default-features --features "portal,inject,clipboard"

clippy: ## Lint with clippy, warnings are errors
	$(CARGO) clippy --workspace --all-targets -- -D warnings

test: ## Run the test suite
	$(CARGO) test --workspace

check: fmt-check clippy test ## Format check, lint and test

ci: check release ## What CI runs

clean: ## Remove build artifacts
	$(CARGO) clean

################################################################################
## Release artifacts
################################################################################

# The suffix a release asset carries: what it runs on, not what it was built on.
# Cross-building (the release pipeline builds the Intel Mac binaries on Apple
# silicon) needs the target spelled out; empty means "this machine".
TARGET    ?=
CARGO_TARGET := $(if $(TARGET),--target $(TARGET))
RELEASE_DIR  := target/$(if $(TARGET),$(TARGET)/)release

UNAME_S   := $(shell uname -s)
UNAME_M   := $(shell uname -m)
OS        := $(if $(filter Darwin,$(UNAME_S)),macos,linux)
ARCH      := $(if $(filter aarch64 arm64,$(UNAME_M)),arm64,amd64)
SUFFIX    ?= $(OS)-$(ARCH)
VERSION   ?= $(shell git describe --tags --always --dirty 2>/dev/null | sed 's/^v//' || echo 0.0.0)
DIST_DIR  := dist
DIST_NAME := $(BIN_NAME)-$(VERSION)-$(SUFFIX)
STAGE     := $(DIST_DIR)/$(DIST_NAME)

dist: release ## Release tarball for this machine, under dist/
	@rm -rf "$(STAGE)"
	@mkdir -p "$(STAGE)"
	@install -m 0755 "$(RELEASE_DIR)/$(BIN_NAME)" "$(STAGE)/"

	@install -m 0644 README.md LICENSE "$(STAGE)/"
	@if [ "$(OS)" = "linux" ]; then \
		install -m 0644 packaging/$(APP_ID).desktop "$(STAGE)/"; \
		install -m 0644 docs/img/icon/$(APP_ID).svg "$(STAGE)/"; \
	fi
	@tar -czf "$(DIST_DIR)/$(DIST_NAME).tar.gz" -C "$(DIST_DIR)" "$(DIST_NAME)"
	@rm -rf "$(STAGE)"
	@echo "$(DIST_DIR)/$(DIST_NAME).tar.gz"

clean-dist: ## Remove release tarballs
	@rm -rf "$(DIST_DIR)"

################################################################################
## Install
################################################################################

install: release ## Install both binaries, the desktop entry and the icon
	@mkdir -p "$(BINDIR)" "$(APPDIR)" "$(ICONDIR)"
	@install -m 0755 "$(RELEASE_DIR)/$(BIN_NAME)" "$(BINDIR)/$(BIN_NAME)"

	@sed 's|^Exec=.*|Exec=$(BINDIR)/$(BIN_NAME)|' packaging/$(APP_ID).desktop > "$(APPDIR)/$(APP_ID).desktop"
	@install -m 0644 docs/img/icon/$(APP_ID).svg "$(ICONDIR)/$(APP_ID).svg"
	@command -v update-desktop-database >/dev/null && update-desktop-database "$(APPDIR)" || true
	@command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -f -t "$(HOME)/.local/share/icons/hicolor" 2>/dev/null || true
	@echo "installed $(BINDIR)/$(BIN_NAME)"
	@echo "the desktop entry is required for the GlobalShortcuts portal:"
	@echo "  $(APPDIR)/$(APP_ID).desktop"

uninstall: ## Remove everything installed by `make install`
	@rm -f "$(BINDIR)/$(BIN_NAME)" "$(APPDIR)/$(APP_ID).desktop" "$(ICONDIR)/$(APP_ID).svg"
	@echo "removed $(BIN_NAME)"

reinstall: uninstall install ## Reinstall

################################################################################
## Environment
################################################################################

deps: ## Toolchain components used by the quality targets
	rustup component add rustfmt clippy
