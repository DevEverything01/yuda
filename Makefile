APP := yuda
PREFIX ?= /usr/local

.PHONY: build run install uninstall clean lint fmt ui test test-linux test-daemon check hooks smoke

build:
	cargo build --release --features daemon

run:
	cargo run --features daemon -- --help

install: build
	install -Dm755 target/release/$(APP) $(DESTDIR)$(PREFIX)/bin/$(APP)
	install -Dm644 packaging/yuda.service $(DESTDIR)$(PREFIX)/lib/systemd/user/yuda.service
	install -Dm644 examples/config.example.toml $(DESTDIR)$(PREFIX)/share/$(APP)/config.example.toml

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/$(APP)
	rm -f $(DESTDIR)$(PREFIX)/lib/systemd/user/yuda.service
	rm -rf $(DESTDIR)$(PREFIX)/share/$(APP)

clean:
	cargo clean

lint:
	cargo clippy --locked --all-targets -- -D warnings

test:
	cargo test --locked

test-linux:
	cargo test --locked --features linux-runtime

test-daemon:
	cargo test --locked --features daemon

check: fmt lint test

smoke:
	scripts/perf_smoke.sh

hooks:
	git config core.hooksPath scripts

fmt:
	cargo fmt --check

ui:
	python3 ui/serve.py
