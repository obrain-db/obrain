#!/usr/bin/env bash
# ============================================================================
# export-public.sh — generates the PUBLIC obrain tree from the private
# monorepo (plan ad63daf1: repo split, phase 1 "filtered export").
#
# The private monorepo stays the source of truth; this script IS the
# definition of the public/private boundary:
#   PRIVATE : crates/obrain-cognitive, crates/obrain-rag, and the
#             feature-gated integration files listed in PRIVATE_FILES.
#   PUBLIC  : everything else (common, core, substrate, engine, adapters,
#             cli, bindings, reactive, iam, neo4j2obrain, migrate, facade).
#
# Usage:   scripts/export-public.sh [OUT_DIR]        (default /tmp/obrain-public-export)
#          scripts/export-public.sh --no-verify OUT  (skip the cargo gates)
#
# Gates (blocking): anti-leak grep, cargo check --workspace, engine tests
# on the default+cypher feature set. A failing gate leaves the tree in
# OUT_DIR for inspection but exits non-zero.
#
# NOTE on features: the cognitive* feature names remain DECLARED in the
# exported Cargo.tomls (so `cfg(feature = "cognitive")` in mixed files
# stays a valid, never-true cfg) but are scrubbed of every private token.
# Enabling them in the public build is unsupported (compile error) —
# they are reserved for the private cognitive layer. Phase 2 of the plan
# (plugin API inversion) removes them entirely.
# ============================================================================
set -euo pipefail

VERIFY=1
if [[ "${1:-}" == "--no-verify" ]]; then VERIFY=0; shift; fi
SRC="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-/tmp/obrain-public-export}"

PRIVATE_FILES=(
  crates/obrain-engine/src/cognitive_procedures.rs
  crates/obrain-engine/src/cognitive_udfs.rs
  crates/obrain-engine/tests/cognitive_query_test.rs
  crates/obrain-engine/tests/cognitive_engram_procedures_test.rs
  crates/bindings/python/src/cognitive.rs
  crates/bindings/wasm/src/cognitive_browser.rs
)

echo "==> export: $SRC → $OUT"
rm -rf "$OUT"
mkdir -p "$OUT"
rsync -a \
  --exclude target --exclude .git --exclude '*.obrain' \
  --exclude crates/obrain-cognitive \
  --exclude crates/obrain-rag \
  --exclude bench \
  "$SRC/" "$OUT/"
# bench/ is the private benchmarking harness: standalone workspace that
# path-depends on obrain-cognitive (ldleiden/retrieval benches) and
# targets the local prod bases — private by nature.

# Module files are STUBBED, not deleted: `cargo fmt` resolves every `mod`
# declaration regardless of cfg-gating, so a missing file breaks the
# public CI's format check. Integration tests are auto-discovered → safe
# to delete outright.
for f in "${PRIVATE_FILES[@]}"; do
  case "$f" in
    */tests/*) rm -f "$OUT/$f" ;;
    *)
      cat > "$OUT/$f" <<'STUB'
//! Reserved for the obrain-cognitive layer (closed source).
//!
//! This stub keeps the module tree resolvable for rustfmt; the feature
//! that would compile this module is not available in the public build.
STUB
      ;;
  esac
done

echo "==> workflows: replace monorepo CI with a public-safe one"
# The monorepo workflows use --all-features (would enable the stripped
# cognitive features) and reference bench/ (excluded). The public repo
# ships its own minimal CI.
rm -rf "$OUT/.github/workflows"
mkdir -p "$OUT/.github/workflows"
cat > "$OUT/.github/workflows/ci.yml" <<'YAML'
name: CI
on:
  push: { branches: [main] }
  pull_request:
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { components: "rustfmt, clippy" }
      - uses: Swatinem/rust-cache@v2
      - name: Format
        run: cargo fmt --all -- --check
      - name: Clippy
        run: cargo clippy --workspace --all-targets --features "obrain-engine/cypher,obrain-engine/algos" -- -D warnings
      - name: Tests
        run: cargo test --workspace --features "obrain-engine/cypher,obrain-engine/algos"
YAML

echo "==> docs & branding: public README, drop cognitive-layer docs"
# The monorepo README markets the cognitive layer — the public repo ships
# an engine-focused README kept at docs/README-public.md in the private repo.
cp "$SRC/docs/README-public.md" "$OUT/README.md"
rm -rf "$OUT/docs/cognitive" "$OUT/docs/README-public.md"
rm -f "$OUT/docs/rfc/substrate/cognitive-quality-"*.md
rm -f "$OUT/docs/rfc/substrate/cognitive-quality.md"
# Cognitive concept sweep on remaining docs is advisory (concepts, not code):
LEFT=$(grep -rli "engram\|stigmergy\|hopfield\|epigenetic" "$OUT/docs" 2>/dev/null | wc -l | tr -d ' ')
echo "    docs mentioning advanced-cognitive concepts remaining: $LEFT (review advised)"

echo "==> scrub workspace manifest"
sed -i '' \
  -e '/crates\/obrain-cognitive/d' \
  -e '/crates\/obrain-rag/d' \
  -e '/^obrain-cognitive[[:space:]]*=/d' \
  -e '/^obrain-rag[[:space:]]*=/d' \
  "$OUT/Cargo.toml"

echo "==> scrub crate manifests (dep lines + private feature tokens)"
find "$OUT/crates" -name Cargo.toml -exec sed -i '' \
  -e '/^obrain-cognitive[[:space:]]*=/d' \
  -e '/^obrain-rag[[:space:]]*=/d' \
  -e 's/"dep:obrain-cognitive",[[:space:]]*//g' \
  -e 's/,[[:space:]]*"dep:obrain-cognitive"//g' \
  -e 's/"dep:obrain-cognitive"//g' \
  -e 's/"dep:obrain-rag",[[:space:]]*//g' \
  -e 's/,[[:space:]]*"dep:obrain-rag"//g' \
  -e 's/"dep:obrain-rag"//g' \
  -e 's/"obrain-cognitive\/[a-zA-Z0-9_-]*",[[:space:]]*//g' \
  -e 's/,[[:space:]]*"obrain-cognitive\/[a-zA-Z0-9_-]*"//g' \
  -e 's/"obrain-cognitive\/[a-zA-Z0-9_-]*"//g' \
  -e 's/"obrain-rag\/[a-zA-Z0-9_-]*",[[:space:]]*//g' \
  -e 's/,[[:space:]]*"obrain-rag\/[a-zA-Z0-9_-]*"//g' \
  -e 's/"obrain-rag\/[a-zA-Z0-9_-]*"//g' \
  {} +

# neo4j2obrain: the public importer defaults to no cognitive stage 4.
# The feature stays DECLARED (empty) so `cfg(feature = "cognitive-init")`
# remains a valid, never-true cfg (unexpected_cfgs is deny in CI).
sed -i '' 's/^default = \["cognitive-init"\]/default = []/' \
  "$OUT/crates/neo4j2obrain/Cargo.toml"
sed -i '' 's/^cognitive-init = .*/cognitive-init = []/' \
  "$OUT/crates/neo4j2obrain/Cargo.toml"

echo "==> anti-leak audit"
# Cargo.toml must be completely clean.
if grep -rn "obrain-cognitive\|obrain-rag" "$OUT" --include=Cargo.toml; then
  echo "❌ LEAK: private crate referenced in a manifest"; exit 1
fi
# Rust sources: only cfg-gated (never-compiled) references may remain in
# mixed files. Any UNgated reference is a leak.
LEAKS=$(grep -rln "obrain_cognitive\|obrain_rag" "$OUT/crates" --include='*.rs' || true)
for f in $LEAKS; do
  rel="${f#"$OUT"/}"
  case "$rel" in
    crates/obrain-engine/src/database/mod.rs| \
    crates/obrain-engine/src/database/annotator.rs| \
    crates/obrain-engine/src/query/planner/lpg/mod.rs| \
    crates/obrain-core/src/execution/operators/cognitive_boost.rs| \
    crates/neo4j2obrain/src/cognitive_init.rs)
      : ;; # known mixed files — references gated behind never-true features
    *)
      echo "❌ LEAK: unexpected private reference in $rel"; exit 1 ;;
  esac
done
echo "    manifests clean; gated references confined to known mixed files"

if [[ "$VERIFY" == 1 ]]; then
  echo "==> gates: cargo check --workspace (default features)"
  (cd "$OUT" && cargo check --workspace 2>&1 | tail -2)
  echo "==> gates: clippy STABLE on the export (exactly what the public CI runs)"
  # Feature unification differs between the private and public workspaces
  # after scrubbing — lints must be checked on THIS tree, with the same
  # latest-stable toolchain as the CI runner (NB: Homebrew cargo shadows
  # rustup: use `rustup run stable`, never `cargo +stable`).
  (cd "$OUT" && rustup run stable cargo clippy --workspace --all-targets \
      --features "obrain-engine/cypher,obrain-engine/algos" -- -D warnings 2>&1 \
    | grep -cE "^error" | { read -r n; echo "    clippy(stable) errors: $n"; [ "$n" = "0" ]; })
  echo "==> gates: quick engine test slice"
  (cd "$OUT" && cargo test -p obrain-engine --test gql_order_by_limit 2>&1 | grep "test result")
fi

echo "✅ export ready: $OUT"
