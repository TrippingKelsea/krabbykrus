.PHONY: dev release test

FEATURES ?= enhanced
CARGO_FEATURE_FLAGS := --no-default-features --features $(FEATURES)

dev:
	cargo build $(CARGO_FEATURE_FLAGS)

release:
	cargo build --release $(CARGO_FEATURE_FLAGS)

test:
	cargo test --workspace --lib --bins --tests $(CARGO_FEATURE_FLAGS)
