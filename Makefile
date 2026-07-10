UNAME_S := $(shell uname -s)

ifeq ($(UNAME_S),Darwin)
  PREFIX ?= $(HOME)/.local/bless
else
  PREFIX ?= $(HOME)/.local
endif

INSTALL_NAME ?= bls

BINDIR ?= $(PREFIX)/bin
MANDIR ?= $(PREFIX)/share/man
CARGO ?= cargo
INSTALL ?= install
GZIP ?= gzip

.PHONY: all release man install uninstall clean platform

all: release man

platform:
	@printf 'platform=%s\n' '$(UNAME_S)'
	@printf 'prefix=%s\n' '$(PREFIX)'
	@printf 'install_name=%s\n' '$(INSTALL_NAME)'

release:
	$(CARGO) build --release

man:
	COMMAND_NAME='$(INSTALL_NAME)' ./scripts/generate-man.sh

install: release man
	$(INSTALL) -d "$(DESTDIR)$(BINDIR)"
	$(INSTALL) -m 0755 target/release/bless "$(DESTDIR)$(BINDIR)/$(INSTALL_NAME)"
	$(INSTALL) -d "$(DESTDIR)$(MANDIR)/man1"
	$(GZIP) -cn doc/$(INSTALL_NAME).1 > "$(DESTDIR)$(MANDIR)/man1/$(INSTALL_NAME).1.gz"
	chmod 0644 "$(DESTDIR)$(MANDIR)/man1/$(INSTALL_NAME).1.gz"

uninstall:
	rm -f "$(DESTDIR)$(BINDIR)/$(INSTALL_NAME)" "$(DESTDIR)$(MANDIR)/man1/$(INSTALL_NAME).1.gz"

clean:
	$(CARGO) clean
