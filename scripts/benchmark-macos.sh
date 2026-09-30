#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
sample_dir="${1:-$PWD/.validation/bench-inputs}"
cargo run -p bluepanda-core --example make_bench_fixtures -- "$sample_dir"
cargo build -p bluepanda-change --release
for sample in "$sample_dir"/*.litematic; do
  name="$(basename "$sample" .litematic)"
  mkdir -p "$sample_dir/output-$name"
  echo "=== $name ==="
  /usr/bin/time -l target/release/bluepanda-change "$sample" --output-dir "$sample_dir/output-$name" --overwrite
done
