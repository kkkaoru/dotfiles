#!/bin/sh

set -eu

check_file_coverage() {
  awk '$7 ~ /%$/ && $10 ~ /%$/ { \
    found=1; if ($7+0 < 95 || $10+0 < 95) failed=1 \
  } END { exit (!found || failed) }' "$1"
}

coverage_directory=.build/coverage
test_binary=$coverage_directory/SleepControlCoreTests
raw_profile=$coverage_directory/default.profraw
profile=$coverage_directory/default.profdata

mkdir -p "$coverage_directory"
swiftc \
  -parse-as-library \
  -swift-version 6 \
  -warnings-as-errors \
  -strict-concurrency=complete \
  -warn-concurrency \
  -warn-implicit-overrides \
  -warn-soft-deprecated \
  -profile-generate \
  -profile-coverage-mapping \
  Sources/SleepControlCore/*.swift \
  Tests/SleepControlCoreTests/*.swift \
  -o "$test_binary"
LLVM_PROFILE_FILE="$raw_profile" "$test_binary"
xcrun llvm-profdata merge -sparse "$raw_profile" -o "$profile"
coverage=$(
  xcrun llvm-cov report "$test_binary" \
    -instr-profile "$profile" \
    -ignore-filename-regex='Tests/' \
    | awk '/TOTAL/ {sub(/%/, "", $10); print $10}'
)
awk -v coverage="$coverage" 'BEGIN { exit !(coverage >= 95) }'
printf 'Swift core line coverage: %s%%\n' "$coverage"
xcrun llvm-cov report "$test_binary" -instr-profile "$profile" \
  Sources/SleepControlCore/BatteryRecord.swift Sources/SleepControlCore/BatterySleepController.swift \
  Sources/SleepControlCore/BatterySleepReading.swift Sources/SleepControlCore/BatterySleepSettings.swift \
  Sources/SleepControlCore/ShortcutSettingsStore.swift \
  Sources/SleepControlCore/ShortcutSettingsStore+BatterySleep.swift \
  > "$coverage_directory/battery-report.txt"
check_file_coverage "$coverage_directory/battery-report.txt"

swift build --enable-code-coverage -Xswiftc -warnings-as-errors --product SleepControlSnapshots
LLVM_PROFILE_FILE="$coverage_directory/menu.profraw" .build/debug/SleepControlSnapshots \
  Resources "$coverage_directory/menu-snapshots"
xcrun llvm-profdata merge -sparse "$coverage_directory/menu.profraw" \
  -o "$coverage_directory/menu.profdata"
xcrun llvm-cov report .build/debug/SleepControlSnapshots \
  -instr-profile "$coverage_directory/menu.profdata" \
  Sources/SleepControlUI/MenuTitleWorkaround.swift \
  > "$coverage_directory/menu-report.txt"
check_file_coverage "$coverage_directory/menu-report.txt"
printf 'Menu title workaround function and line coverage: >=95%%\n'
xcrun llvm-cov report .build/debug/SleepControlSnapshots \
  -instr-profile "$coverage_directory/menu.profdata" \
  Sources/SleepControlUI/BatterySleepSettingsView.swift \
  Sources/SleepControlUI/ShortcutSettingsView.swift Sources/SleepControlUI/ShortcutSettingsStrings.swift \
  > "$coverage_directory/battery-ui-report.txt"
check_file_coverage "$coverage_directory/battery-ui-report.txt"

system_binary=$coverage_directory/SleepControlSystemTests
swiftc -parse-as-library -swift-version 6 -warnings-as-errors -strict-concurrency=complete \
  -warn-concurrency -warn-implicit-overrides -warn-soft-deprecated \
  -profile-generate -profile-coverage-mapping \
  Sources/SleepControlCore/*.swift \
  Sources/SleepControl/PowerCommand.swift Sources/SleepControl/SystemBatterySleepClient.swift \
  Sources/SleepControl/SleepSettingsError.swift Sources/SleepControl/BatterySleepEvents.swift \
  Tests/SleepControlSystemTests/*.swift Tests/SleepControlCoreTests/TestError.swift \
  -o "$system_binary"
LLVM_PROFILE_FILE="$coverage_directory/system.profraw" "$system_binary"
xcrun llvm-profdata merge -sparse "$coverage_directory/system.profraw" \
  -o "$coverage_directory/system.profdata"
xcrun llvm-cov report "$system_binary" -instr-profile "$coverage_directory/system.profdata" \
  Sources/SleepControl/PowerCommand.swift Sources/SleepControl/SystemBatterySleepClient.swift \
  Sources/SleepControl/BatterySleepEvents.swift \
  > "$coverage_directory/system-report.txt"
check_file_coverage "$coverage_directory/system-report.txt"
printf 'Battery sleep per-file function and line coverage: >=95%%\n'
