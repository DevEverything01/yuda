APP := yuda
PREFIX ?= /usr/local
BINDIR := $(PREFIX)/bin
SHAREDIR := $(PREFIX)/share/$(APP)
FEATURES ?= daemon

.PHONY: build run install uninstall clean lint fmt test test-linux test-daemon check hooks smoke e2e

build:
	cargo build --release --locked --features $(FEATURES)

run:
	cargo run --features $(FEATURES) -- daemon

# install 不依赖 build：sudo make install 时不应以 root 重新编译。
install:
	@test -x target/release/$(APP) || { echo "先运行 make build"; exit 1; }
	install -Dm755 target/release/$(APP) $(DESTDIR)$(BINDIR)/$(APP)
	install -Dm755 omarchy/bin/omarchy-dictation-yuda $(DESTDIR)$(BINDIR)/omarchy-dictation-yuda
	install -Dm755 omarchy/bin/omarchy-install-dictation-yuda $(DESTDIR)$(BINDIR)/omarchy-install-dictation-yuda
	install -Dm755 omarchy/bin/omarchy-remove-dictation-yuda $(DESTDIR)$(BINDIR)/omarchy-remove-dictation-yuda
	install -dm755 $(DESTDIR)$(PREFIX)/lib/systemd/user
	sed 's|@BINDIR@|$(BINDIR)|' packaging/yuda.service > $(DESTDIR)$(PREFIX)/lib/systemd/user/yuda.service
	install -Dm644 examples/config.example.toml $(DESTDIR)$(SHAREDIR)/config.example.toml
	install -Dm644 omarchy/hypr/shortcuts.lua $(DESTDIR)$(SHAREDIR)/shortcuts.lua
	if [ -d omarchy/plugin/yuda.dictation ]; then \
		install -dm755 $(DESTDIR)$(SHAREDIR)/omarchy-plugin; \
		cp -r omarchy/plugin/yuda.dictation $(DESTDIR)$(SHAREDIR)/omarchy-plugin/; \
	fi

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/$(APP)
	rm -f $(DESTDIR)$(BINDIR)/omarchy-dictation-yuda $(DESTDIR)$(BINDIR)/omarchy-install-dictation-yuda $(DESTDIR)$(BINDIR)/omarchy-remove-dictation-yuda
	rm -f $(DESTDIR)$(PREFIX)/lib/systemd/user/yuda.service
	rm -rf $(DESTDIR)$(SHAREDIR)

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

e2e:
	scripts/omarchy-e2e

hooks:
	git config core.hooksPath scripts

fmt:
	cargo fmt --check
