PKG_CONFIG ?= pkg-config
CC ?= gcc
PLATFORM ?= linux
EXE_SUFFIX :=

ifeq ($(PLATFORM),windows)
EXE_SUFFIX := .exe
endif

TARGET_DIR := build/linux
TARGET := $(TARGET_DIR)/ubar

.PHONY: all clean run assets

all: assets
	cargo build
	cp target/debug/ubar $(TARGET)

build:
	mkdir -p $(TARGET_DIR)/assets/newtab $(TARGET_DIR)/assets/pages

assets: build
	cp -r assets/newtab $(TARGET_DIR)/assets/
	cp -r assets/pages $(TARGET_DIR)/assets/

run: all
	./$(TARGET)

clean:
	rm -rf build target
