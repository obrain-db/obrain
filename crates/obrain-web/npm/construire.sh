#!/usr/bin/env bash
# Construit le paquet npm @obrain-db/wasm avec wasm-pack (version épinglée), installe
# le package.json publié à la place de celui que wasm-pack génère, puis liste le contenu
# de l'archive sans rien publier. Usage, depuis la racine du dépôt :
#   crates/obrain-web/npm/construire.sh
set -euo pipefail
WASM_PACK_VERSION=0.13.1
RACINE="$(cd "$(dirname "$0")/../../.." && pwd)"
CRATE="$RACINE/crates/obrain-web"
SORTIE="$CRATE/pkg-npm"

version_wasm_pack="$(wasm-pack --version 2>/dev/null | awk '{print $2}' || true)"
if [ "$version_wasm_pack" != "$WASM_PACK_VERSION" ]; then
  echo "wasm-pack $WASM_PACK_VERSION requis (trouvé : ${version_wasm_pack:-aucun})." >&2
  echo "Installation : cargo install --locked --version $WASM_PACK_VERSION wasm-pack" >&2
  exit 1
fi

# Profil complet, comme le paquet de test de la CI. --out-dir est relatif au crate.
wasm-pack build "$CRATE" --target web --release --out-dir pkg-npm -- --features complet
cp "$CRATE/npm/package.json" "$SORTIE/package.json"
cp "$CRATE/README.md" "$SORTIE/README.md"
cd "$SORTIE"
npm pack --dry-run
