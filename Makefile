# The vault defaults to ~/.config/quirenote/config.toml (or native first-run setup).
VAULT ?=
ADDR ?= 127.0.0.1:8080
CARGO_FLAGS ?= --locked
CARGO_JOBS ?= 1
SERVER_TARGET_DIR ?= rust/target
GUI_TARGET_DIR ?= gui/target
PREFIX ?= $(HOME)/.local

.PHONY: dev dev-api dev-gui build build-dev check fmt install clean

build:
	cargo build $(CARGO_FLAGS) --release --manifest-path rust/Cargo.toml --target-dir $(SERVER_TARGET_DIR) -j $(CARGO_JOBS)
	cargo build $(CARGO_FLAGS) --release --manifest-path gui/Cargo.toml --target-dir $(GUI_TARGET_DIR) -j $(CARGO_JOBS)
	install -Dm755 $(SERVER_TARGET_DIR)/release/quirenote-rs bin/quirenote
	install -Dm755 $(GUI_TARGET_DIR)/release/quirenote-gui bin/quirenote-gui

build-dev:
	cargo build $(CARGO_FLAGS) --manifest-path rust/Cargo.toml --target-dir $(SERVER_TARGET_DIR) -j $(CARGO_JOBS)
	cargo build $(CARGO_FLAGS) --manifest-path gui/Cargo.toml --target-dir $(GUI_TARGET_DIR) -j $(CARGO_JOBS)
	install -Dm755 $(SERVER_TARGET_DIR)/debug/quirenote-rs bin/quirenote-dev
	install -Dm755 $(GUI_TARGET_DIR)/debug/quirenote-gui bin/quirenote-gui-dev

dev: build-dev
	./scripts/dev.sh $(if $(VAULT),-vault "$(VAULT)") -addr $(ADDR) -debug

dev-api:
	cargo run $(CARGO_FLAGS) --manifest-path rust/Cargo.toml --target-dir $(SERVER_TARGET_DIR) -j $(CARGO_JOBS) -- $(if $(VAULT),-vault "$(VAULT)") -addr $(ADDR) -debug

dev-gui:
	cargo run $(CARGO_FLAGS) --manifest-path gui/Cargo.toml --target-dir $(GUI_TARGET_DIR) -j $(CARGO_JOBS) -- --server http://$(ADDR)

check:
	cargo fmt --manifest-path rust/Cargo.toml -- --check
	cargo fmt --manifest-path gui/Cargo.toml -- --check
	cargo clippy $(CARGO_FLAGS) --manifest-path rust/Cargo.toml --target-dir $(SERVER_TARGET_DIR) -j $(CARGO_JOBS) --all-targets -- -D warnings
	cargo clippy $(CARGO_FLAGS) --manifest-path gui/Cargo.toml --target-dir $(GUI_TARGET_DIR) -j $(CARGO_JOBS) --all-targets -- -D warnings
	cargo test $(CARGO_FLAGS) --manifest-path rust/Cargo.toml --target-dir $(SERVER_TARGET_DIR) -j $(CARGO_JOBS)
	cargo test $(CARGO_FLAGS) --manifest-path gui/Cargo.toml --target-dir $(GUI_TARGET_DIR) -j $(CARGO_JOBS)
	bash -n deploy/quirenote-app scripts/dev.sh

fmt:
	cargo fmt --manifest-path rust/Cargo.toml
	cargo fmt --manifest-path gui/Cargo.toml

install: build
	install -Dm755 bin/quirenote $(PREFIX)/bin/quirenote
	install -Dm755 bin/quirenote-gui $(PREFIX)/bin/quirenote-gui
	install -Dm755 deploy/quirenote-app $(PREFIX)/bin/quirenote-app
	install -Dm644 deploy/quirenote.service $(HOME)/.config/systemd/user/quirenote.service
	install -Dm644 assets/icon-512.png $(PREFIX)/share/icons/hicolor/512x512/apps/quirenote.png
	install -d $(PREFIX)/share/applications
	sed 's|^Icon=.*|Icon=$(PREFIX)/share/icons/hicolor/512x512/apps/quirenote.png|' deploy/quirenote.desktop > $(PREFIX)/share/applications/quirenote.desktop
	systemctl --user daemon-reload
	@echo "Installed the Rust backend and native interface. Start with quirenote-app."

clean:
	cargo clean --manifest-path rust/Cargo.toml --target-dir $(SERVER_TARGET_DIR)
	cargo clean --manifest-path gui/Cargo.toml --target-dir $(GUI_TARGET_DIR)
	rm -f bin/quirenote bin/quirenote-gui bin/quirenote-dev bin/quirenote-gui-dev
