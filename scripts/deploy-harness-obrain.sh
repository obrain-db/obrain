#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# deploy-harness-obrain.sh — fait du checkpoint obrain le NOUVEAU POINT DE
# RÉFÉRENCE du harness (prod :3082 + dev :3083), sans redémarrer les serveurs.
#
# Principes : versionné + backup unique + remplacement atomique + VÉRIFICATION
# (protocole bridge sur copie réelle du store) + rollback en une commande.
# Les bridges DÉJÀ lancés gardent leur ancien binaire (POSIX) ; les nouveaux
# spawns utilisent la référence. R7 respecté (pas de redémarrage :3082).
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
NEW="${1:-$ROOT/target/release/obrain}"
SHARE="${HOME}/.local/share/obrain-harness/bin"
BIN="$SHARE/obrain"
STORE="${2:-$ROOT/target/bench/real/rt-prod}"   # copie RO du store prod (défaut)
BAK="$SHARE/obrain.bak-20260904"
TAG="v0.0.1-zorder-2026-09-04"

[ -x "$NEW" ] || { echo "ERREUR: release absente: $NEW (cargo build --release -p obrain-cli)"; exit 1; }
mkdir -p "$SHARE"
# 1) backup unique
if [ ! -f "$BAK" ] && [ -f "$BIN" ]; then cp -p "$BIN" "$BAK"; echo "backup → $BAK"; fi
# 2) copie versionnée + remplacement atomique
cp "$NEW" "$SHARE/obrain-$TAG"
cp "$NEW" "$BIN.new" && sync && mv -f "$BIN.new" "$BIN" && chmod +x "$BIN"
echo "déployé : $BIN ← $TAG ($(sha256sum "$BIN" | cut -c1-16)…)"
# 3) identité
"$BIN" --version
cmp -s "$BIN" "$NEW" && echo "intégrité : binaire == release ✓"
# 4) VÉRIFICATION protocole bridge (store RO, sonde initialize + sessions_list)
if [ -d "$STORE" ]; then
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
                '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"sessions_list","arguments":{}}}' \
    | OBRAIN_STORE="$STORE" timeout 90 "$BIN" bridge 2>/dev/null | grep -c '"result"' | grep -q '^2$' \
    && echo "protocole bridge ✓ (initialize + tools/call sessions_list répondent sur $STORE)" \
    || { echo "ÉCHEC sonde bridge — rollback:"; cp -f "$BAK" "$BIN"; exit 1; }
else
  echo "AVERTISSEMENT: copie store absente ($STORE) — sonde sautée (rollback possible: cp $BAK $BIN)"
fi
echo "OK — référence harness = $TAG (backup: $BAK ; rollback: cp $BAK $BIN)"
