#!/usr/bin/env bash
# Liste les liens RELATIFS cassés des fichiers Markdown (README.md, docs/**).
# Sortie vide et code 0 = rien de cassé ; sinon une ligne « fichier -> cible »
# par lien cassé et code 1. Les URL (http/https/mailto) et les ancres seules
# (#…) ne sont pas vérifiées.
set -uo pipefail
cd "$(dirname "$0")/.."
casses=""
while IFS= read -r fichier; do
  dossier=$(dirname "$fichier")
  while IFS= read -r cible; do
    [ -z "$cible" ] && continue
    case "$cible" in http://*|https://*|mailto:*) continue;; esac
    if [ ! -e "$dossier/$cible" ] && [ ! -e "$cible" ]; then
      casses+="$fichier -> $cible"$'\n'
    fi
  done < <(grep -oE '\]\([^)]+\)' "$fichier" 2>/dev/null | sed -E 's/^\]\(//; s/\)$//; s/#.*$//; s/ .*$//' | sort -u)
done < <(find README.md docs -name '*.md' 2>/dev/null | sort)
if [ -n "$casses" ]; then printf '%s' "$casses"; exit 1; fi
