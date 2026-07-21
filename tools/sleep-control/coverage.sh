#!/bin/sh

set -eu

coverage_directory=.build/coverage
test_binary=$coverage_directory/SleepControlCoreTests
raw_profile=$coverage_directory/default.profraw
profile=$coverage_directory/default.profdata

mkdir -p "$coverage_directory"
swiftc \
  -parse-as-library \
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
