#!/usr/bin/env bash
# ÉQUIVALENCE DES RÉSULTATS — un chantier de perf doit prouver qu'il rend LES
# MÊMES LIGNES, pas seulement qu'il va plus vite.
#
# ── POURQUOI LE COMPTE, ET PAS L'ÉCHANTILLON ────────────────────────────────
#
# Les requêtes qui ont motivé le chantier finissent par `LIMIT 30` SANS
# `ORDER BY` : rien n'ordonne, donc *n'importe quel* sous-ensemble des lignes
# qui satisfont le prédicat est une réponse juste. Deux moteurs corrects peuvent
# donc légitimement rendre des échantillons DIFFÉRENTS dès que le plan change —
# et c'est précisément ce qu'on vient de changer.
//
# La preuve d'équivalence ne peut donc pas être « les 30 lignes sont les mêmes ».
# Elle est le COMPTE COMPLET, sans `LIMIT` : deux moteurs qui rendent le même
# ENSEMBLE rendent le même compte, quel que soit l'ordre de lecture. C'est
# l'invariant, et c'est lui qui fait échouer le script.
#
# L'échantillon limité est comparé AUSSI — mais son verdict est un INDICE, pas
# une faute : une divergence y est signalée sans faire échouer, et c'est le
# compte qu'il faut alors regarder.
#
# ── USAGE ───────────────────────────────────────────────────────────────────
#
#   scripts/equiv-resultats.sh [STORE]
#
#   STORE     défaut : le banc figé ~/bench-stores/runtime-dev-snapshot-20260909.obrain
#   TEMOIN    défaut : le binaire installé du harnais (ANTÉRIEUR au chantier)
#   CANDIDAT  défaut : target/release/obrain (le chantier)
#   TOURS     défaut : 3 (le minimum est retenu, comme le banc de cellules)
#
# Sortie : une ligne par prédicat, puis un verdict global. Code de sortie 1 dès
# qu'un COMPTE diverge — c'est le seul échec qui compte.

set -uo pipefail

STORE="${1:-$HOME/bench-stores/runtime-dev-snapshot-20260909.obrain}"
RACINE="$(cd "$(dirname "$0")/.." && pwd)"
TEMOIN="${TEMOIN:-/home/triviere/.local/share/obrain-harness/bin/obrain}"
CANDIDAT="${CANDIDAT:-$RACINE/target/release/obrain}"
TOURS="${TOURS:-3}"

for b in "$TEMOIN" "$CANDIDAT"; do
    [[ -x "$b" ]] || { echo "binaire absent : $b" >&2; exit 2; }
done
[[ -d "$STORE" ]] || { echo "store absent : $STORE" >&2; exit 2; }

TIMEOUT="${TIMEOUT:-600}"

# Rend « millisecondes » depuis la ligne de temps du CLI (1.23ms / 205us / 1.5s).
en_ms() {
    case "$1" in
        *us) awk -v v="${1%us}" 'BEGIN{printf "%.3f", v/1000}' ;;
        *ms) printf '%s' "${1%ms}" ;;
        *s)  awk -v v="${1%s}"  'BEGIN{printf "%.3f", v*1000}' ;;
        *)   printf '0' ;;
    esac
}

# Exécute une requête : le JSON sur stdout, les millisecondes sur stdout aussi
# (dernière ligne, préfixée « # »). Le bruit de journal reste sur stderr.
executer() {
    local bin="$1" q="$2"
    timeout "$TIMEOUT" "$bin" query "$STORE" "$q" --format json --timing 2>/dev/null \
        | { cat; }
}

# Millisecondes de la dernière exécution, via la sortie --timing (sur stderr).
ms_de() {
    local bin="$1" q="$2"
    timeout "$TIMEOUT" "$bin" query "$STORE" "$q" --timing --format table 2>/dev/null \
        | grep -oE '\([0-9.]+(us|ms|s)\)' | tail -1 | tr -d '()'
}

# Le minimum sur TOURS exécutions — même convention que le banc de cellules.
min_ms() {
    local bin="$1" q="$2" t=() v
    for _ in $(seq "$TOURS"); do
        v=$(ms_de "$bin" "$q")
        [[ -n "$v" ]] && t+=("$(en_ms "$v")")
    done
    [[ ${#t[@]} -eq 0 ]] && { printf 'ÉCHEC'; return; }
    printf '%s\n' "${t[@]}" | sort -g | head -1
}

# Compare deux sorties JSON : compte exact des lignes, puis multiensemble.
comparer() {
    python3 - "$1" "$2" <<'PY'
import json, sys, collections
def charge(p):
    try:
        return json.load(open(p, encoding="utf-8"))
    except Exception as e:
        return {"__erreur__": str(e)}
a, b = charge(sys.argv[1]), charge(sys.argv[2])
if isinstance(a, dict) or isinstance(b, dict):
    print("ERREUR", json.dumps(a)[:120], "|", json.dumps(b)[:120]); raise SystemExit
ka = collections.Counter(json.dumps(r, sort_keys=True, ensure_ascii=False) for r in a)
kb = collections.Counter(json.dumps(r, sort_keys=True, ensure_ascii=False) for r in b)
if ka == kb:
    print("EGALES", len(a))
else:
    print("DIFFERENTES", len(a), len(b), "| première ligne absente du candidat :",
          (next(iter((ka - kb)), "-")[:160] if ka - kb else "-"))
PY
}

ligne() {  # nom, requete_compte, requete_reelle
    local nom="$1" qc="$2" qr="$3"
    local ca cb ta tb verdict
    executer "$TEMOIN"   "$qc" > /tmp/equiv-temoin.json
    executer "$CANDIDAT" "$qc" > /tmp/equiv-candidat.json
    read -r verdict _ <<<"$(comparer /tmp/equiv-temoin.json /tmp/equiv-candidat.json)"
    # Le compte est la SEULE valeur de la SEULE ligne : on ne code pas en dur le
    # nom de la colonne (`countnonnull(...)`, `count(*)` selon la forme écrite).
    valeur_unique() {
        python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
print(next(iter(d[0].values())) if d else "0")' "$1" 2>/dev/null || echo '?'
    }
    ca=$(valeur_unique /tmp/equiv-temoin.json)
    cb=$(valeur_unique /tmp/equiv-candidat.json)

    if [[ "$verdict" != "EGALES" ]]; then
        printf '%-28s COMPTES DIVERGENTS  témoin=%s candidat=%s  ✗\n' "$nom" "$ca" "$cb"
        ECHECS=$((ECHECS+1))
        return
    fi

    ta=$(min_ms "$TEMOIN" "$qr"); tb=$(min_ms "$CANDIDAT" "$qr")
    # L'échantillon limité : indice, jamais faute (aucun ORDER BY).
    executer "$TEMOIN"   "$qr" > /tmp/equiv-temoin-lim.json
    executer "$CANDIDAT" "$qr" > /tmp/equiv-candidat-lim.json
    local votr
    read -r votr _ <<<"$(comparer /tmp/equiv-temoin-lim.json /tmp/equiv-candidat-lim.json)"

    awk -v n="$nom" -v ca="$ca" -v ta="$ta" -v tb="$tb" -v ech="$votr" 'BEGIN{
        gain = (tb+0 > 0) ? ta/tb : 0;
        printf "%-28s compte %8s  égal   témoin %8.1f ms → candidat %8.1f ms  (%4.1f×)  échantillon:%s\n",
               n, ca, ta, tb, gain, (ech=="EGALES" ? "identique" : "DIFFÉRENT (indice)")
    }'
}

ECHECS=0
echo "store   : $STORE"
echo "témoin  : $TEMOIN"
echo "candidat: $CANDIDAT"
echo "tours   : $TOURS (minimum retenu)"
echo
printf '%-28s %s\n' "prédicat" "verdict (le COMPTE fait foi)"
printf '%s\n' "---------------------------------------------------------------------------------------------"

# Les prédicats sont ceux du journal MCP du 2026-09-12 : c'est cette plainte qui
# a ouvert le chantier. Le compte est sans LIMIT ; la requête réelle le reprend.
ligne "name CONTAINS iam" \
    "MATCH (n) WHERE n.name CONTAINS 'iam' OR n.name CONTAINS 'IAM' OR n.qualified_name CONTAINS 'iam' RETURN count(n)" \
    "MATCH (n) WHERE n.name CONTAINS 'iam' OR n.name CONTAINS 'IAM' OR n.qualified_name CONTAINS 'iam' RETURN n.qualified_name, labels(n) LIMIT 30"

ligne "4 termes auth/token" \
    "MATCH (n) WHERE n.name CONTAINS 'auth' OR n.name CONTAINS 'Auth' OR n.name CONTAINS 'token' OR n.name CONTAINS 'Token' RETURN count(n)" \
    "MATCH (n) WHERE n.name CONTAINS 'auth' OR n.name CONTAINS 'Auth' OR n.name CONTAINS 'token' OR n.name CONTAINS 'Token' RETURN n.qualified_name, labels(n) LIMIT 30"

ligne "STARTS WITH préfixe" \
    "MATCH (n) WHERE n.qualified_name STARTS WITH 'home.triviere.projects.banc_obrain.crates.obrain-iam' RETURN count(n)" \
    "MATCH (n) WHERE n.qualified_name STARTS WITH 'home.triviere.projects.banc_obrain.crates.obrain-iam' RETURN n.qualified_name, labels(n) LIMIT 60"

ligne "File CONTAINS obrain-iam" \
    "MATCH (f:File) WHERE f.qualified_name CONTAINS 'obrain-iam' RETURN count(f)" \
    "MATCH (f:File) WHERE f.qualified_name CONTAINS 'obrain-iam' RETURN f.qualified_name LIMIT 40"

ligne "préfixe + labels (OU)" \
    "MATCH (n) WHERE n.qualified_name STARTS WITH 'home.triviere.projects.banc_obrain.crates.obrain-iam.src' AND (n:Module OR n:Struct OR n:Enum OR n:Trait OR n:Class OR n:Interface OR n:Type) RETURN count(n)" \
    "MATCH (n) WHERE n.qualified_name STARTS WITH 'home.triviere.projects.banc_obrain.crates.obrain-iam.src' AND (n:Module OR n:Struct OR n:Enum OR n:Trait OR n:Class OR n:Interface OR n:Type) RETURN n.qualified_name, labels(n) LIMIT 40"

ligne "arêtes traversantes" \
    "MATCH (a)-[r]->(b) WHERE a.qualified_name CONTAINS 'obrain-iam' AND NOT b.qualified_name CONTAINS 'obrain-iam' RETURN count(*)" \
    "MATCH (a)-[r]->(b) WHERE a.qualified_name CONTAINS 'obrain-iam' AND NOT b.qualified_name CONTAINS 'obrain-iam' RETURN a.qualified_name, type(r), b.qualified_name LIMIT 40"

echo
if [[ $ECHECS -eq 0 ]]; then
    echo "VERDICT : tous les COMPTES sont égaux — le candidat rend le même ENSEMBLE de lignes."
    echo "          (Un échantillon marqué DIFFÉRENT n'est pas une faute : sans ORDER BY,"
    echo "           un plan différent a le droit de rendre d'autres lignes parmi les mêmes.)"
    exit 0
fi
echo "VERDICT : $ECHECS prédicat(s) DIVERGENT(S) — le candidat ne rend pas le même ensemble. ✗"
exit 1
