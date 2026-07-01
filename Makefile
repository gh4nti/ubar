PKG_CONFIG ?= pkg-config
CC ?= gcc

TARGET := build/ubar
SRC := src/main.c src/browser_app.c
CFLAGS += -Wall -Wextra -Wpedantic $(shell $(PKG_CONFIG) --cflags gtk4 webkitgtk-6.0) -Iinclude
LDLIBS += $(shell $(PKG_CONFIG) --libs gtk4 webkitgtk-6.0)

.PHONY: all clean run

all: $(TARGET) build/assets/newtab/index.html build/assets/newtab/styles.css build/assets/newtab/app.js

$(TARGET): $(SRC) include/browser_app.h | build
	$(CC) $(CFLAGS) $(SRC) $(LDLIBS) -o $@

build:
	mkdir -p build/assets/newtab

build/assets/newtab/index.html: assets/newtab/index.html | build
	cp $< $@

build/assets/newtab/styles.css: assets/newtab/styles.css | build
	cp $< $@

build/assets/newtab/app.js: assets/newtab/app.js | build
	cp $< $@

run: all
	./$(TARGET)

clean:
	rm -rf build
