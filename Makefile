BIN_DIR := $(HOME)/.local/bin
BINARY := target/release/edgee

.PHONY: build install

build:
	cargo build --release

install: build
	mkdir -p $(BIN_DIR)
	ln -sf $(CURDIR)/$(BINARY) $(BIN_DIR)/edgee
