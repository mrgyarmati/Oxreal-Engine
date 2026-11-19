#!/usr/bin/env bash
# Checks that the Vulkan device(s) reported by `vulkaninfo` support every feature in
# required-vulkan-features.txt, and report a Vulkan 1.3+ device (ADR-0012, ADR-0016).
#
# Usage: tools/ci/check-vulkan-features.sh      (needs `vulkaninfo` on PATH)
# Exit status: 0 if all features are supported, 1 if any is missing, 2 on setup errors.
set -euo pipefail

list="$(dirname "$0")/required-vulkan-features.txt"
if ! command -v vulkaninfo >/dev/null 2>&1; then
    echo "error: vulkaninfo not found (install vulkan-tools, or the Vulkan SDK)" >&2
    exit 2
fi

summary="$(vulkaninfo --summary 2>/dev/null | tr -d '\r')"
full="$(vulkaninfo 2>/dev/null | tr -d '\r')"

echo "Devices:"
grep -E '^\s*(deviceName|apiVersion|driverName|driverInfo)\s*=' <<<"$summary" | sed 's/^\s*/  /'

# Every listed device must be Vulkan 1.3 or newer (apiVersion = 1.MINOR.PATCH).
api_ok=1
while read -r version; do
    minor="$(cut -d. -f2 <<<"$version")"
    if [ "$(cut -d. -f1 <<<"$version")" -lt 1 ] || { [ "$(cut -d. -f1 <<<"$version")" -eq 1 ] && [ "$minor" -lt 3 ]; }; then
        echo "  device apiVersion $version is below the Vulkan 1.3 baseline"
        api_ok=0
    fi
done < <(grep -E '^\s*apiVersion\s*=' <<<"$summary" | sed -E 's/.*= *([0-9]+\.[0-9]+\.[0-9]+).*/\1/')

echo "Required features:"
missing=0
while IFS= read -r feature; do
    feature="${feature%%#*}"
    feature="$(tr -d '[:space:]' <<<"$feature")"
    [ -z "$feature" ] && continue
    if grep -Eq "^\s*${feature}\s*=\s*true\s*$" <<<"$full"; then
        echo "  ok       $feature"
    else
        echo "  MISSING  $feature"
        missing=1
    fi
done < "$list"

if [ "$missing" -ne 0 ] || [ "$api_ok" -ne 1 ]; then
    echo "FAILED: the device does not meet Oxreal's Vulkan requirements (ADR-0012, ADR-0016)" >&2
    exit 1
fi
echo "OK: all required Vulkan features are supported"
