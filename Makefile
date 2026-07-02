PKG_CONFIG ?= pkg-config
CC ?= gcc
PLATFORM ?= linux
EXE_SUFFIX :=

ifeq ($(PLATFORM),windows)
EXE_SUFFIX := .exe
endif

TARGET_DIR := build/$(PLATFORM)
TARGET := $(TARGET_DIR)/ubar$(EXE_SUFFIX)
SRC := src/main.c src/browser_app.c src/browser_state.c
CFLAGS += -Wall -Wextra -Wpedantic $(shell $(PKG_CONFIG) --cflags gtk4 webkitgtk-6.0) -Iinclude
LDLIBS += $(shell $(PKG_CONFIG) --libs gtk4 webkitgtk-6.0)

.PHONY: all clean run linux windows

all: $(TARGET) \
	$(TARGET_DIR)/assets/newtab/index.html \
	$(TARGET_DIR)/assets/newtab/styles.css \
	$(TARGET_DIR)/assets/newtab/app.js \
	$(TARGET_DIR)/assets/pages/history/index.html \
	$(TARGET_DIR)/assets/pages/history/app.js \
	$(TARGET_DIR)/assets/pages/bookmarks/index.html \
	$(TARGET_DIR)/assets/pages/bookmarks/app.js \
	$(TARGET_DIR)/assets/pages/settings/index.html \
	$(TARGET_DIR)/assets/pages/settings/app.js \
	$(TARGET_DIR)/assets/pages/shared/page.css

$(TARGET): $(SRC) include/browser_app.h include/browser_state.h | build
	$(CC) $(CFLAGS) $(SRC) $(LDLIBS) -o $@

build:
	mkdir -p $(TARGET_DIR)/assets/newtab $(TARGET_DIR)/assets/pages/history $(TARGET_DIR)/assets/pages/bookmarks $(TARGET_DIR)/assets/pages/settings $(TARGET_DIR)/assets/pages/shared

$(TARGET_DIR)/assets/newtab/index.html: assets/newtab/index.html | build
	cp $< $@

$(TARGET_DIR)/assets/newtab/styles.css: assets/newtab/styles.css | build
	cp $< $@

$(TARGET_DIR)/assets/newtab/app.js: assets/newtab/app.js | build
	cp $< $@

$(TARGET_DIR)/assets/pages/history/index.html: assets/pages/history/index.html | build
	cp $< $@

$(TARGET_DIR)/assets/pages/history/app.js: assets/pages/history/app.js | build
	cp $< $@

$(TARGET_DIR)/assets/pages/bookmarks/index.html: assets/pages/bookmarks/index.html | build
	cp $< $@

$(TARGET_DIR)/assets/pages/bookmarks/app.js: assets/pages/bookmarks/app.js | build
	cp $< $@

$(TARGET_DIR)/assets/pages/settings/index.html: assets/pages/settings/index.html | build
	cp $< $@

$(TARGET_DIR)/assets/pages/settings/app.js: assets/pages/settings/app.js | build
	cp $< $@

$(TARGET_DIR)/assets/pages/shared/page.css: assets/pages/shared/page.css | build
	cp $< $@

run: all
	./$(TARGET)

linux:
	$(MAKE) PLATFORM=linux all

windows:
	$(MAKE) PLATFORM=windows all

clean:
	rm -rf build
