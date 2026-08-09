.PHONY: build format install lint release test verify restart status uninstall

PRODUCT = mac-wifi-stability
RELEASE_BINARY = .build/release/$(PRODUCT)
INSTALL_BINARY = $(HOME)/.local/bin/$(PRODUCT)
LAUNCH_AGENT_LABEL = com.kkkaoru.mac-wifi-stability
LAUNCH_AGENT_TEMPLATE = launchd/$(LAUNCH_AGENT_LABEL).plist.in
INSTALL_PLIST = $(HOME)/Library/LaunchAgents/$(LAUNCH_AGENT_LABEL).plist
LAUNCH_DOMAIN = gui/$(shell id -u)
RELEASE_FLAGS = -O -whole-module-optimization
SWIFTC_STRICT_FLAGS = -warnings-as-errors -strict-concurrency=complete -warn-concurrency -warn-implicit-overrides -warn-soft-deprecated
SWIFTPM_STRICT_FLAGS = $(foreach flag,$(SWIFTC_STRICT_FLAGS),-Xswiftc $(flag))
SWIFT_TOOLCHAIN_LIB = $(abspath $(dir $(shell xcrun --find swift))/../lib)
SWIFTLINT = DYLD_FRAMEWORK_PATH="$(SWIFT_TOOLCHAIN_LIB)" swiftlint

build:
	swift build --product $(PRODUCT) $(SWIFTPM_STRICT_FLAGS)

format:
	swift format --configuration .swift-format --in-place --recursive Package.swift Sources Tests

install: release
	mkdir -p "$(dir $(INSTALL_BINARY))" "$(dir $(INSTALL_PLIST))" "$(HOME)/Library/Logs"
	install -m 755 "$(RELEASE_BINARY)" "$(INSTALL_BINARY)"
	sed \
		-e 's|@EXECUTABLE@|$(INSTALL_BINARY)|g' \
		-e 's|@HOME@|$(HOME)|g' \
		-e 's|@LAUNCHD_LOG@|$(HOME)/Library/Logs/com.kkkaoru.mac-wifi-stability.launchd.log|g' \
		"$(LAUNCH_AGENT_TEMPLATE)" > "$(INSTALL_PLIST)"
	plutil -lint "$(INSTALL_PLIST)"
	touch "$(HOME)/Library/Logs/com.kkkaoru.mac-wifi-stability.launchd.log"
	chmod 600 "$(HOME)/Library/Logs/com.kkkaoru.mac-wifi-stability.launchd.log"
	launchctl bootout "$(LAUNCH_DOMAIN)/$(LAUNCH_AGENT_LABEL)" 2>/dev/null || true
	launchctl enable "$(LAUNCH_DOMAIN)/$(LAUNCH_AGENT_LABEL)"
	launchctl bootstrap "$(LAUNCH_DOMAIN)" "$(INSTALL_PLIST)"
	launchctl print "$(LAUNCH_DOMAIN)/$(LAUNCH_AGENT_LABEL)"

lint:
	swift format lint --configuration .swift-format --strict --recursive Package.swift Sources Tests
	$(SWIFTLINT) lint --strict --config .swiftlint.yml
	swift build --product $(PRODUCT) $(SWIFTPM_STRICT_FLAGS)
	plutil -lint "$(LAUNCH_AGENT_TEMPLATE)"

release:
	swift build -c release --product $(PRODUCT) $(SWIFTPM_STRICT_FLAGS)

restart:
	launchctl kickstart -k "$(LAUNCH_DOMAIN)/$(LAUNCH_AGENT_LABEL)"

status:
	launchctl print "$(LAUNCH_DOMAIN)/$(LAUNCH_AGENT_LABEL)"

test:
	swift run $(SWIFTPM_STRICT_FLAGS) MacWiFiStabilityCoreTests

uninstall:
	launchctl bootout "$(LAUNCH_DOMAIN)/$(LAUNCH_AGENT_LABEL)" 2>/dev/null || true
	rm -f "$(INSTALL_PLIST)" "$(INSTALL_BINARY)"

verify: lint test release
