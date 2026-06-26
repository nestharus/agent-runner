#!/usr/bin/env bash
set -euo pipefail

PROVIDERS_TOML="${1:-$HOME/.config/oulipoly-agent-runner/providers.toml}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd -- "$SCRIPT_DIR/../../.." && pwd)}"

if [[ ! -f "$PROVIDERS_TOML" ]]; then
  echo "providers.toml not found: $PROVIDERS_TOML" >&2
  exit 1
fi

if [[ ! -d "$REPO_ROOT/crates/oulipoly-config" ]]; then
  echo "could not find oulipoly-config under REPO_ROOT=$REPO_ROOT" >&2
  exit 1
fi

timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
backup="$PROVIDERS_TOML.$timestamp.bak"
tmp="$PROVIDERS_TOML.$timestamp.tmp"
validator_dir="$(mktemp -d)"

cleanup() {
  rm -f -- "$tmp"
  rm -rf -- "$validator_dir"
}
trap cleanup EXIT

cp -- "$PROVIDERS_TOML" "$backup"
cp -- "$PROVIDERS_TOML" "$tmp"
echo "backup: $backup"

python3 - "$tmp" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
text = path.read_text()
lines = text.splitlines(keepends=True)

targets = {
    "opencode": ("opencode-cwd ~/.local/share/opencode", Path("~/.local/share/opencode")),
    "opencode3": ("opencode-cwd ~/.opencode3/opencode", Path("~/.opencode3/opencode")),
    "opencode4": ("opencode-cwd ~/.opencode4/opencode", Path("~/.opencode4/opencode")),
    "opencode5": ("opencode-cwd ~/.opencode5/opencode", Path("~/.opencode5/opencode")),
    "opencode6": ("opencode-cwd ~/.opencode6/opencode", Path("~/.opencode6/opencode")),
}
optional_missing = {"opencode6"}


def block_bounds(account: str) -> tuple[int, int] | None:
    header = f"[{account}.session_storage]"
    start = None
    for idx, line in enumerate(lines):
        if line.strip() == header:
            start = idx
            break
    if start is None:
        return None
    end = len(lines)
    for idx in range(start + 1, len(lines)):
        stripped = lines[idx].strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            end = idx
            break
    return start, end


changed = []
already = []
skipped = []
errors = []

for account, (command, db_dir) in targets.items():
    bounds = block_bounds(account)
    if bounds is None:
        if account in optional_missing:
            skipped.append(f"{account}: session_storage block missing")
            continue
        errors.append(f"{account}: missing [{account}.session_storage] block")
        continue

    start, end = bounds
    desired_line = f'cwd_script = "{command}"'
    cwd_line_idx = None
    for idx in range(start + 1, end):
        if lines[idx].lstrip().startswith("cwd_script ="):
            cwd_line_idx = idx
            break

    if cwd_line_idx is None:
        errors.append(f"{account}: missing cwd_script line")
        continue

    current = lines[cwd_line_idx].strip()
    if current == desired_line:
        already.append(account)
        continue

    if current != 'cwd_script = "/bin/false"':
        errors.append(f"{account}: unexpected cwd_script value: {current}")
        continue

    db_path = db_dir.expanduser() / "opencode.db"
    if not db_path.is_file():
        errors.append(f"{account}: refusing to apply unverified cwd_script; missing {db_path}")
        continue

    newline = "\n" if lines[cwd_line_idx].endswith("\n") else ""
    indent = lines[cwd_line_idx][: len(lines[cwd_line_idx]) - len(lines[cwd_line_idx].lstrip())]
    lines[cwd_line_idx] = f"{indent}{desired_line}{newline}"
    changed.append(account)

if errors:
    for error in errors:
        print(error, file=sys.stderr)
    sys.exit(1)

path.write_text("".join(lines))
for account in changed:
    print(f"updated: {account}")
for account in already:
    print(f"already: {account}")
for message in skipped:
    print(f"skipped: {message}")
PY

mkdir -p -- "$validator_dir/src"
cat > "$validator_dir/Cargo.toml" <<EOF
[package]
name = "oulipoly-provider-config-validate"
version = "0.0.0"
edition = "2024"

[dependencies]
oulipoly-config = { path = "$REPO_ROOT/crates/oulipoly-config" }
EOF

cat > "$validator_dir/src/main.rs" <<'RS'
use oulipoly_config::repositories::{FilesystemProvidersConfigRepository, ProvidersConfigRepository};
use std::path::Path;

fn main() {
    let path = std::env::args().nth(1).expect("providers.toml path");
    let repo = FilesystemProvidersConfigRepository;
    match repo.load_providers(Path::new(&path)) {
        Ok(config) => println!("providers.toml parsed: {} provider entries", config.entries.len()),
        Err(error) => {
            eprintln!("providers.toml parse failed: {error}");
            std::process::exit(1);
        }
    }
}
RS

echo "validating edited temp file"
cargo run --quiet --manifest-path "$validator_dir/Cargo.toml" -- "$tmp"

mv -- "$tmp" "$PROVIDERS_TOML"

echo "validating installed file"
cargo run --quiet --manifest-path "$validator_dir/Cargo.toml" -- "$PROVIDERS_TOML"
echo "cwd backfill complete"
