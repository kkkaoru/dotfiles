.PHONY: build coverage format install lint release restart status test uninstall verify

PRODUCT = lid-display-watcher
RELEASE_BINARY = .build/release/$(PRODUCT)
# Leave less than 1 KiB of headroom over the audited 33,824-byte executable.
MAX_RELEASE_BYTES = 34816
LAUNCH_AGENT_LABEL = com.kkkaoru.lid-display-watcher
LAUNCH_AGENT_TEMPLATE = launchd/$(LAUNCH_AGENT_LABEL).plist.in
INSTALL_BINARY = $(HOME)/.local/bin/$(PRODUCT)
INSTALL_PLIST = $(HOME)/Library/LaunchAgents/$(LAUNCH_AGENT_LABEL).plist
LAUNCH_DOMAIN = gui/$(shell id -u)
RELEASE_FLAGS = -O -whole-module-optimization
LINKER_FLAGS = -Xlinker -dead_strip -Xlinker -no_data_const -Xlinker -no_function_starts -Xlinker -exported_symbol -Xlinker _main
SWIFTC_STRICT_FLAGS = -warnings-as-errors -strict-concurrency=complete -warn-concurrency -warn-implicit-overrides -warn-soft-deprecated
SWIFTPM_STRICT_FLAGS = $(foreach flag,$(SWIFTC_STRICT_FLAGS),-Xswiftc $(flag))
SWIFT_TOOLCHAIN_LIB = $(abspath $(dir $(shell xcrun --find swift))/../lib)
SWIFTLINT = DYLD_FRAMEWORK_PATH="$(SWIFT_TOOLCHAIN_LIB)" swiftlint

build:
	swift build --product $(PRODUCT) $(SWIFTPM_STRICT_FLAGS)

coverage:
	./coverage.sh

format:
	swift format --configuration .swift-format --in-place --recursive Package.swift Sources Tests

install: release
	mkdir -p "$(dir $(INSTALL_BINARY))" "$(dir $(INSTALL_PLIST))"
	install -m 755 "$(RELEASE_BINARY)" "$(INSTALL_BINARY)"
	sed 's|@EXECUTABLE@|$(INSTALL_BINARY)|g' "$(LAUNCH_AGENT_TEMPLATE)" > "$(INSTALL_PLIST)"
	plutil -lint "$(INSTALL_PLIST)"
	launchctl bootout "$(LAUNCH_DOMAIN)/$(LAUNCH_AGENT_LABEL)" 2>/dev/null || true
	launchctl bootstrap "$(LAUNCH_DOMAIN)" "$(INSTALL_PLIST)"
	launchctl enable "$(LAUNCH_DOMAIN)/$(LAUNCH_AGENT_LABEL)"
	launchctl kickstart -k "$(LAUNCH_DOMAIN)/$(LAUNCH_AGENT_LABEL)"

lint:
	swift format lint --configuration .swift-format --strict --recursive Package.swift Sources Tests
	$(SWIFTLINT) lint --strict --config .swiftlint.yml
	swift build --product $(PRODUCT) $(SWIFTPM_STRICT_FLAGS)
	plutil -lint "$(LAUNCH_AGENT_TEMPLATE)"

release:
	mkdir -p .build/release
	swiftc $(RELEASE_FLAGS) $(SWIFTC_STRICT_FLAGS) $(LINKER_FLAGS) -module-name LidDisplayWatcher -framework CoreFoundation -framework IOKit Sources/LidDisplayWatcherCore/LidStateTransition.swift Sources/LidDisplayWatcher/*.swift -o $(RELEASE_BINARY)
	strip -N $(RELEASE_BINARY)
	@bytes=$$(stat -f %z "$(RELEASE_BINARY)"); \
		if [ "$$bytes" -gt "$(MAX_RELEASE_BYTES)" ]; then \
			echo "Release size $$bytes exceeds $(MAX_RELEASE_BYTES) bytes" >&2; \
			exit 1; \
		fi; \
		echo "Release size: $$bytes / $(MAX_RELEASE_BYTES) bytes"

restart:
	launchctl kickstart -k "$(LAUNCH_DOMAIN)/$(LAUNCH_AGENT_LABEL)"

status:
	launchctl print "$(LAUNCH_DOMAIN)/$(LAUNCH_AGENT_LABEL)"

test:
	swift run $(SWIFTPM_STRICT_FLAGS) LidDisplayWatcherCoreTests

uninstall:
	launchctl bootout "$(LAUNCH_DOMAIN)/$(LAUNCH_AGENT_LABEL)" 2>/dev/null || true
	rm -f "$(INSTALL_PLIST)" "$(INSTALL_BINARY)"

verify: lint test coverage release
