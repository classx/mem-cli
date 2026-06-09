BIN := mem-cli
PREFIX ?= $(HOME)/.local

.PHONY: all build release test lint fmt fmt-check clippy run clean install

all: build

build:
	cargo build

release:
	cargo build --release

test:
	cargo test

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

clippy:
	cargo clippy -- -D warnings

lint: fmt-check clippy

run:
	cargo run -- $(ARGS)

clean:
	cargo clean

install: release
	install -Dm755 target/release/$(BIN) $(PREFIX)/bin/$(BIN)
