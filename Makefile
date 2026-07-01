PKG_CONFIG ?= pkg-config
CC ?= gcc

TARGET := build/ubar
SRC := src/main.c src/browser_app.c src/browser_state.c
CFLAGS += -Wall -Wextra -Wpedantic $(shell $(PKG_CONFIG) --cflags gtk4 webkitgtk-6.0) -Iinclude
LDLIBS += $(shell $(PKG_CONFIG) --libs gtk4 webkitgtk-6.0)

.PHONY: all clean run

all: $(TARGET) \
	build/assets/newtab/index.html \
	build/assets/newtab/styles.css \
	build/assets/newtab/app.js \
	build/assets/pages/history/index.html \
	build/assets/pages/history/app.js \
	build/assets/pages/bookmarks/index.html \
	build/assets/pages/bookmarks/app.js \
	build/assets/pages/settings/index.html \
	build/assets/pages/settings/app.js \
	build/assets/pages/shared/page.css

$(TARGET): $(SRC) include/browser_app.h | build
	$(CC) $(CFLAGS) $(SRC) $(LDLIBS) -o $@

build:
	mkdir -p build/assets/newtab build/assets/pages/history build/assets/pages/bookmarks build/assets/pages/settings build/assets/pages/shared

build/assets/newtab/index.html: assets/newtab/index.html | build
	cp $< $@

build/assets/newtab/styles.css: assets/newtab/styles.css | build
	cp $< $@

build/assets/newtab/app.js: assets/newtab/app.js | build
	cp $< $@

build/assets/pages/history/index.html: assets/pages/history/index.html | build
	cp $< $@

build/assets/pages/history/app.js: assets/pages/history/app.js | build
	cp $< $@

build/assets/pages/bookmarks/index.html: assets/pages/bookmarks/index.html | build
	cp $< $@

build/assets/pages/bookmarks/app.js: assets/pages/bookmarks/app.js | build
	cp $< $@

build/assets/pages/settings/index.html: assets/pages/settings/index.html | build
	cp $< $@

build/assets/pages/settings/app.js: assets/pages/settings/app.js | build
	cp $< $@

build/assets/pages/shared/page.css: assets/pages/shared/page.css | build
	cp $< $@

run: all
	./$(TARGET)

clean:
	rm -rf build
