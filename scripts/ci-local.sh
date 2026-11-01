#!/bin/bash
# Local CI check script - run this before making a PR to avoid surprises
# Usage: ./scripts/ci-local.sh [--quick]

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

QUICK_MODE=false
if [[ "$1" == "--quick" ]]; then
    QUICK_MODE=true
    echo -e "${YELLOW}Running in quick mode (skipping release tests)${NC}"
fi

echo -e "\n${YELLOW}========================================${NC}"
echo -e "${YELLOW}  Local CI Check${NC}"
echo -e "${YELLOW}========================================${NC}\n"

# Track failures
FAILURES=()

run_check() {
    local name="$1"
    local cmd="$2"
    echo -e "\n${YELLOW}[$name]${NC} Running..."
    if eval "$cmd"; then
        echo -e "${GREEN}[$name] PASSED${NC}"
    else
        echo -e "${RED}[$name] FAILED${NC}"
        FAILURES+=("$name")
    fi
}

# 1. Rust formatting
run_check "Format" "cargo fmt --all -- --check"

# 2. Clippy
run_check "Clippy" "cargo clippy --all-targets --all-features -- -D warnings"

# 3. Documentation : doit se CONSTRUIRE (c'est ce que docs.rs fera). Les liens
#    intra-doc cassés restent des avertissements : mesuré le 01/11/2026, 156
#    sur le workspace (tâche V1-RUSTDOC) — un -D warnings ici ne serait qu'un
#    barrage rouge en permanence, pas un portillon.
run_check "Docs" "rm -rf target/doc && cargo doc --no-deps --all-features"

# 4. Rust tests
run_check "Rust Tests" "cargo test --all-features --workspace"

# 5. Rust tests (release) - skip in quick mode
if [[ "$QUICK_MODE" == false ]]; then
    run_check "Rust Tests (Release)" "cargo test --all-features --workspace --release"
fi

# 6. Python : aucun binding dans ce dépôt (v0.0.1). Les paquets
#    packages/obrain-cli-{npm,python} sont des wrappers du CLI, testés à part.

# Summary
echo -e "\n${YELLOW}========================================${NC}"
echo -e "${YELLOW}  Summary${NC}"
echo -e "${YELLOW}========================================${NC}\n"

if [[ ${#FAILURES[@]} -eq 0 ]]; then
    echo -e "${GREEN}All checks passed!${NC}"
    exit 0
else
    echo -e "${RED}Failed checks:${NC}"
    for f in "${FAILURES[@]}"; do
        echo -e "  ${RED}- $f${NC}"
    done
    exit 1
fi
