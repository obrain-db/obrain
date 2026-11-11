#!/usr/bin/env bash
# Vérifie que chaque crate publiable porte les métadonnées crates.io
# (description, license, repository, readme, keywords, categories,
# rust-version) et que ses dépendances internes ont une version.
# POURQUOI pas `cargo package` / `cargo publish --dry-run` pour tous : ils
# exigent que les dépendances internes soient DÉJÀ sur crates.io (mesuré
# le 01/11/2026 : « no matching package named `obrain-common` found »).
# Seul le premier de l'ordre de publication (obrain-common) se packagera
# avant la publication réelle, séquentielle.
set -uo pipefail
cd "$(dirname "$0")/.."
echec=0
for c in obrain-common obrain-core obrain-substrate obrain-adapters obrain-reactive obrain-engine obrain-cognitive obrain; do
  m="crates/$c/Cargo.toml"
  for cle in description license readme keywords categories repository rust-version; do
    if ! grep -qE "^$cle(\.workspace)? *=" "$m"; then echo "$c : clé manquante : $cle"; echec=1; fi
  done
  [ -f "crates/$c/README.md" ] || { echo "$c : README.md absent"; echec=1; }
  [ -f "crates/$c/LICENSE" ] || { echo "$c : LICENSE absent"; echec=1; }
  # `obrain-bases-banc` est l'outil de test interne (publish = false). Un chemin
  # SANS version n'est admis pour lui QUE dans une section de dev-dépendances
  # (cargo le retire alors du manifeste publié) ; partout ailleurs, et pour tout
  # autre crate interne, c'est une faute.
  if awk '
    /^\[/ { section = $0 }
    /^obrain-[a-z-]+ *= *\{[^}]*path *=/ && !/version/ {
      if ($0 ~ /^obrain-bases-banc / && section ~ /dev-dependencies\]$/) next
      trouve = 1
    }
    END { exit trouve ? 0 : 1 }' "$m"; then
    echo "$c : dépendance interne par chemin sans version"; echec=1
  fi
done
# Les dépendances internes du workspace portent path + version.
grep -nE '^obrain-[a-z-]+ *= *\{' Cargo.toml | grep -v 'version *=' | grep -q . && { echo "Cargo.toml : dépendance interne sans version"; echec=1; }
[ $echec -eq 0 ] && echo "métadonnées de publication : OK (8 crates)"
exit $echec
