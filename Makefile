APP := yuda
PREFIX ?= /usr/local

.PHONY: build run install uninstall clean lint fmt ui

build:
	cargo build --release

run:
	cargo run -- --help

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
	cargo clippy -- -D warnings

fmt:
	cargo fmt --check

ui:
	python3 ui/serve.py
