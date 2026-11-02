.PHONY: check fmt build

check:
	cargo fmt --all -- --check
	cargo clippy --locked --all-targets -- -D warnings
	cargo test --locked --all-targets

fmt:
	cargo fmt --all

build:
	cargo build --locked --release
