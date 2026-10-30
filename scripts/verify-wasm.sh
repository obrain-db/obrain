#!/usr/bin/env bash
# Vérifie le chemin WASM de bout en bout — ce que `build-wasm.sh` ne fait pas.
#
# POURQUOI CE SCRIPT EXISTE. Le dépôt savait CONSTRUIRE le module wasm, jamais
# le VÉRIFIER : `build-wasm.sh` s'arrête à la taille du binaire. Résultat
# mesuré le 2026-09-17 : le module livré (`crates/bindings/wasm/pkg/`) trappait
# dès `new Database()`, et la suite de tests wasm ne compilait même plus — donc
# personne ne pouvait le voir. Ce script est la garde qui manquait.
#
# Il fait quatre choses, dans cet ordre :
#   1. il construit le module (profil `minimal-size`, features par défaut) ;
#   2. il refuse un module qui lie le panic d'horloge std (mêmes règles que
#      `build-wasm.sh` — dupliquées ici pour que ce script soit autonome) ;
#   3. il EXÉCUTE la suite de tests wasm du binding (wasm-bindgen-test-runner) ;
#   4. il rapporte les comptes, et sort en erreur si un test échoue.
#
# Outillage : le CLI `wasm-bindgen` doit être de la version EXACTE du crate
# (`wasm-bindgen` dans Cargo.lock), sinon il refuse le module. Le script trouve
# donc la bonne version dans le cache de wasm-pack — c'est là qu'elle vit quand
# `~/.cargo/bin/wasm-bindgen` est en retard, ce qui est le cas ici.
#
# Usage : ./scripts/verify-wasm.sh [--no-test]

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

RUN_TESTS=1
[[ "${1:-}" == "--no-test" ]] && RUN_TESTS=0

PROFILE="minimal-size"
WASM_FILE="target/wasm32-unknown-unknown/${PROFILE}/obrain_wasm.wasm"
TEST_CRATE="obrain-wasm"
TEST_TARGET="web"

# --- 0. Outillage : trouver un wasm-bindgen de la version du crate ------------

VERSION_ATTENDUE="$(awk '
    /^name = "wasm-bindgen"$/ { found = 1; next }
    found && /^version = / { gsub(/[",]/, "", $3); print $3; exit }
' Cargo.lock)"

if [[ -z "$VERSION_ATTENDUE" ]]; then
    echo "Error: impossible de lire la version de wasm-bindgen dans Cargo.lock"
    exit 1
fi

trouver_cli() {
    # 1) celui du PATH, s'il est bon
    if command -v wasm-bindgen >/dev/null 2>&1; then
        if wasm-bindgen --version 2>/dev/null | grep -q "$VERSION_ATTENDUE"; then
            command -v wasm-bindgen
            return 0
        fi
    fi
    # 2) le cache de wasm-pack, à la bonne version
    local c
    for c in "$HOME"/.cache/.wasm-pack/wasm-bindgen-*/wasm-bindgen; do
        [[ -x "$c" ]] || continue
        if "$c" --version 2>/dev/null | grep -q "$VERSION_ATTENDUE"; then
            echo "$c"
            return 0
        fi
    done
    return 1
}

CLI="$(trouver_cli || true)"
if [[ -z "$CLI" ]]; then
    echo "Error: aucun wasm-bindgen $VERSION_ATTENDUE (version exigée par Cargo.lock)."
    echo "       PATH : $(command -v wasm-bindgen 2>/dev/null || echo absent)"
    echo "       Cache wasm-pack : $(ls -d "$HOME"/.cache/.wasm-pack/wasm-bindgen-*/ 2>/dev/null || echo absent)"
    echo "       Remede : cargo install wasm-bindgen-cli --version $VERSION_ATTENDUE"
    exit 1
fi

BINDGEN_DIR="$(dirname "$CLI")"
RUNNER="$BINDGEN_DIR/wasm-bindgen-test-runner"

# Le runner doit être visible comme `wasm-bindgen`/`wasm-bindgen-test-runner`
# dans le PATH : c'est par ces noms que le harnais de test les appelle.
export PATH="$BINDGEN_DIR:$PATH"

echo "wasm-bindgen : $("$CLI" --version)  ($CLI)"

# --- 1. Construire ------------------------------------------------------------

echo ""
echo "=== 1/3  Construction du module wasm (profil ${PROFILE}) ==="
cargo build --target wasm32-unknown-unknown --profile "$PROFILE" -p "$TEST_CRATE"

if [[ ! -f "$WASM_FILE" ]]; then
    echo "Error: ${WASM_FILE} absent apres construction"
    exit 1
fi
echo "  taille : $(stat -c%s "$WASM_FILE") octets"

# --- 2. Garde horloge ---------------------------------------------------------

echo ""
echo "=== 2/3  Garde horloge (std::time non implemente sur wasm32) ==="
if strings -a "$WASM_FILE" | grep -q "time not implemented on this platform"; then
    echo "Error: le module lie le panic d'horloge std."
    echo "       Remede : web-time (au lieu de std::time::{Instant,SystemTime})."
    if strings -a "$WASM_FILE" | grep -q "unsupported/time"; then
        echo "       Trace : library/std/src/sys/pal/unsupported/time.rs"
    fi
    exit 1
fi
echo "  OK : aucun chemin d'horloge std lie"

# --- 3. Exécuter la suite de tests wasm --------------------------------------

if [[ "$RUN_TESTS" -eq 0 ]]; then
    echo ""
    echo "Tests sautes (--no-test)."
    exit 0
fi

echo ""
echo "=== 3/3  Suite de tests wasm (${TEST_CRATE} --test ${TEST_TARGET}) ==="
if [[ ! -x "$RUNNER" ]]; then
    echo "Error: wasm-bindgen-test-runner absent a cote de $CLI"
    exit 1
fi

LOG="$(mktemp)"
set +e
CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER="$RUNNER" \
    cargo test -p "$TEST_CRATE" --target wasm32-unknown-unknown \
    --test "$TEST_TARGET" 2>&1 | tee "$LOG"
STATUS="${PIPESTATUS[0]}"
set -e

RESULTAT="$(grep -E '^test result' "$LOG" | tail -1 || true)"
echo ""
echo "=== Bilan ==="
echo "  exit cargo  : $STATUS"
echo "  resultat    : ${RESULTAT:-indisponible}"

if [[ "$STATUS" -ne 0 ]]; then
    echo ""
    echo "  Causes de panique, comptees :"
    grep -o 'panicked at [^ ]*' "$LOG" | sort | uniq -c | sed 's/^/    /' || true
    grep -oE 'no filesystem on this platform|time not implemented on this platform' "$LOG" \
        | sort | uniq -c | sed 's/^/    /' || true
    echo ""
    echo "  Journal complet : $LOG"
    exit "$STATUS"
fi

echo "  Suite wasm verte."
