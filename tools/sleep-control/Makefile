.PHONY: build bundle coverage format icon install install-authorization lint open snapshots snapshot-test test uninstall uninstall-authorization verify

PRODUCT = sleep-control
APP_NAME = Sleep Control
APP_BUNDLE = .build/app/$(APP_NAME).app
INSTALL_BUNDLE = $(HOME)/Applications/$(APP_NAME).app
ICON_MASTER = Resources/AppIcon.png
ICON_FILE = Resources/AppIcon.icns
ICONSET = .build/AppIcon.iconset
ENABLED_ICON_MASTER = Resources/AppIconSleepEnabled.png
ENABLED_ICON_FILE = Resources/AppIconSleepEnabled.icns
ENABLED_ICONSET = .build/AppIconSleepEnabled.iconset
SNAPSHOT_COUNT = 6
SUDOERS_USER = $(shell id -un)
SUDOERS_TEMPLATE = sudoers/sleep-control.in
SUDOERS_BUILD = .build/sleep-control.sudoers
SUDOERS_FILE = /etc/sudoers.d/sleep-control
SWIFTC_STRICT_FLAGS = -warnings-as-errors -strict-concurrency=complete -warn-concurrency -warn-implicit-overrides -warn-soft-deprecated
SWIFTPM_STRICT_FLAGS = $(foreach flag,$(SWIFTC_STRICT_FLAGS),-Xswiftc $(flag))
SWIFT_TOOLCHAIN_LIB = $(abspath $(dir $(shell xcrun --find swift))/../lib)
SWIFTLINT = DYLD_FRAMEWORK_PATH="$(SWIFT_TOOLCHAIN_LIB)" swiftlint

build:
	swift build --product $(PRODUCT) $(SWIFTPM_STRICT_FLAGS)

bundle: icon
	swift build -c release --product $(PRODUCT) $(SWIFTPM_STRICT_FLAGS)
	rm -rf "$(APP_BUNDLE)"
	mkdir -p "$(APP_BUNDLE)/Contents/MacOS" "$(APP_BUNDLE)/Contents/Resources"
	install -m 755 ".build/release/$(PRODUCT)" "$(APP_BUNDLE)/Contents/MacOS/SleepControl"
	install -m 644 Resources/Info.plist "$(APP_BUNDLE)/Contents/Info.plist"
	install -m 644 "$(ICON_FILE)" "$(APP_BUNDLE)/Contents/Resources/AppIcon.icns"
	install -m 644 "$(ENABLED_ICON_FILE)" "$(APP_BUNDLE)/Contents/Resources/AppIconSleepEnabled.icns"
	ditto Resources/en.lproj "$(APP_BUNDLE)/Contents/Resources/en.lproj"
	ditto Resources/ja.lproj "$(APP_BUNDLE)/Contents/Resources/ja.lproj"
	plutil -lint "$(APP_BUNDLE)/Contents/Info.plist"
	codesign --force --sign - --timestamp=none "$(APP_BUNDLE)"

coverage:
	./coverage.sh

format:
	swift format --configuration .swift-format --in-place --recursive Package.swift Sources Tests

icon:
	rm -rf "$(ICONSET)" "$(ENABLED_ICONSET)"
	./scripts/build-icon.sh "$(ICON_MASTER)" "$(ICONSET)" "$(ICON_FILE)"
	./scripts/build-icon.sh "$(ENABLED_ICON_MASTER)" "$(ENABLED_ICONSET)" "$(ENABLED_ICON_FILE)"

install: bundle
	mkdir -p "$(HOME)/Applications"
	rm -rf "$(INSTALL_BUNDLE)"
	ditto "$(APP_BUNDLE)" "$(INSTALL_BUNDLE)"
	open "$(INSTALL_BUNDLE)"

install-authorization:
	mkdir -p .build
	@umask 077; sed 's/@USER@/$(SUDOERS_USER)/g' "$(SUDOERS_TEMPLATE)" > "$(SUDOERS_BUILD)"
	/usr/sbin/visudo -cf "$(SUDOERS_BUILD)"
	sudo /usr/bin/install -o root -g wheel -m 440 "$(SUDOERS_BUILD)" "$(SUDOERS_FILE)"
	sudo /usr/sbin/visudo -cf "$(SUDOERS_FILE)"

lint:
	swift format lint --configuration .swift-format --strict --recursive Package.swift Sources Tests
	$(SWIFTLINT) lint --strict --config .swiftlint.yml
	swift build --product $(PRODUCT) $(SWIFTPM_STRICT_FLAGS)
	plutil -lint Resources/Info.plist
	plutil -lint Resources/en.lproj/Localizable.strings Resources/en.lproj/InfoPlist.strings
	plutil -lint Resources/ja.lproj/Localizable.strings Resources/ja.lproj/InfoPlist.strings

open: bundle
	open "$(APP_BUNDLE)"

snapshots:
	rm -rf Snapshots
	swift run $(SWIFTPM_STRICT_FLAGS) SleepControlSnapshots Resources Snapshots

snapshot-test:
	rm -rf .build/snapshot-test
	swift run $(SWIFTPM_STRICT_FLAGS) SleepControlSnapshots Resources .build/snapshot-test
	test "$$(find .build/snapshot-test -type f -name '*.png' | wc -l | tr -d ' ')" = $(SNAPSHOT_COUNT)
	find .build/snapshot-test -type f -name '*.png' -exec test -s {} \;
	@for baseline in Snapshots/*.png; do \
		cmp -s "$$baseline" ".build/snapshot-test/$$(basename "$$baseline")" || exit 1; \
	done

test:
	swift run $(SWIFTPM_STRICT_FLAGS) SleepControlCoreTests

uninstall:
	rm -rf "$(INSTALL_BUNDLE)"

uninstall-authorization:
	sudo rm -f "$(SUDOERS_FILE)"

verify: lint test coverage snapshot-test bundle
