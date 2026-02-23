#!/usr/bin/env bash
# Rename model TOML files to use ~ separator for faceted grouping.
# Usage: ./scripts/migrate-model-names.sh <models-directory>
#
# Example: ./scripts/migrate-model-names.sh ~/.agents/models

set -euo pipefail

DIR="${1:?Usage: $0 <models-directory>}"

if [ ! -d "$DIR" ]; then
    echo "Error: $DIR is not a directory"
    exit 1
fi

# Map: old-name -> new-name (without .toml extension)
declare -A RENAMES=(
    # Claude
    ["claude-haiku"]="claude~haiku"
    ["claude-opus"]="claude~opus"
    ["claude-sonnet"]="claude~sonnet"

    # Gemini Flash
    ["gemini-3-flash-high"]="gemini-3-flash~high"
    ["gemini-3-flash-low"]="gemini-3-flash~low"
    ["gemini-3-flash-medium"]="gemini-3-flash~medium"
    ["gemini-3-flash-minimal"]="gemini-3-flash~minimal"

    # Gemini Pro
    ["gemini-3-pro-high"]="gemini-3-pro~high"
    ["gemini-3-pro-low"]="gemini-3-pro~low"

    # GPT 5.1 Codex Mini
    ["gpt-5.1-codex-mini-high"]="gpt-5.1-codex-mini~high"
    ["gpt-5.1-codex-mini-medium"]="gpt-5.1-codex-mini~medium"

    # GPT 5.2
    ["gpt-5.2-high"]="gpt-5.2~high"
    ["gpt-5.2-low"]="gpt-5.2~low"
    ["gpt-5.2-medium"]="gpt-5.2~medium"
    ["gpt-5.2-none"]="gpt-5.2~none"
    ["gpt-5.2-xhigh"]="gpt-5.2~xhigh"

    # GPT 5.3 Codex
    ["gpt-5.3-codex-high"]="gpt-5.3-codex~high"
    ["gpt-5.3-codex-high2"]="gpt-5.3-codex~high2"
    ["gpt-5.3-codex-low"]="gpt-5.3-codex~low"
    ["gpt-5.3-codex-medium"]="gpt-5.3-codex~medium"
    ["gpt-5.3-codex-xhigh"]="gpt-5.3-codex~xhigh"

    # GPT 5.3 Codex Spark
    ["gpt-5.3-codex-spark-xhigh"]="gpt-5.3-codex-spark~xhigh"

    # glm stays as-is (standalone, no rename needed)
)

renamed=0
skipped=0

for old in "${!RENAMES[@]}"; do
    new="${RENAMES[$old]}"
    src="$DIR/${old}.toml"
    dst="$DIR/${new}.toml"

    if [ ! -f "$src" ]; then
        continue
    fi

    if [ -f "$dst" ]; then
        echo "SKIP: $dst already exists"
        ((skipped++))
        continue
    fi

    mv "$src" "$dst"
    echo "RENAMED: ${old}.toml -> ${new}.toml"
    ((renamed++))
done

echo ""
echo "Done: $renamed renamed, $skipped skipped"
