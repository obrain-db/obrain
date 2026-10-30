#!/usr/bin/env bash
# Banc du COÛT PAR CELLULE de propriété dans le filtre GQL.
#
# POURQUOI CE BANC. Mesuré le 2026-09-12 : sur runtime-dev (405 100 nœuds),
# `MATCH (n) WHERE id(n) >= 0 RETURN 1` — balayage complet, AUCUNE cellule lue —
# coûte 23,4 ms (58 ns/ligne), et le MÊME balayage plus UNE propriété
# (`n.name CONTAINS 'zzqq'`) passe à 421 ms (1,04 µs/ligne). Tout l'écart est
# donc la cellule, pas le balayage. `filter.rs` l'écrit lui-même en clair :
# « Scalar route is ~1 µs per cell ».
#
# Ce banc fige les deux extrémités (balayage nu / balayage + cellules) et les
# requêtes réelles qui ont motivé la mesure, pour que chaque levier soit
# vérifiable AVANT/APRÈS sans rediscuter le protocole.
#
# RÉSULTATS EN RÉGIME STATIONNAIRE (snapshot figé, 347 970 nœuds, dernière des
# 3 exécutions dans UN processus — le profil du bridge persistant) :
#
#   K. balayage nu 0 cellule        19,7 ms    57 ns/ligne
#   L. +1 cellule absente           53,2 ms   153 ns/ligne
#   M. +1 cellule présente         135,4 ms   389 ns/ligne   (~330 ns de lecture + boucle)
#   N. 4 termes OR (même propriété) 217,6 ms  625 ns/ligne   (+79 ns par terme, relu ZÉRO fois)
#   O. name CONTAINS 'iam' (ton log) 113,2 ms 325 ns/ligne
#   P. arêtes, 2 cellules         5 580 ms  16 036 ns/ligne  (domaine du PLAN, pas de la cellule)
#
# CHEMIN INDEX — CE QUI EST MESURÉ, ET CE QUI RESTE (2026-09-13) :
#
#  `obrain index create <store> <propriete>` A ÉTÉ AJOUTÉ : `create_property_index`
#  n'était appelable que depuis l'API Rust (aucune commande, aucune DDL), donc
#  en pratique AUCUN index n'était déclaré — la série ordonnée de runtime-dev
#  est vide (4 096 o) alors que la série d'égalité est bâtie (6,5 Mo).
#
#  GAINS MESURÉS (stores contrôlés de 20 000 nœuds, mêmes requêtes) :
#    propriété NUMÉRIQUE indexée, borne sélective   15,5 ms → 0,041 ms   ×378
#    intervalle numérique à deux bornes               52,1 ms → 0,008 ms   ×6500
#    clef CHAÎNE de 9 octets (≤ 11), intervalle        3,9 ms → 0,028 ms   ×139
#    `STARTS WITH` (jamais réécrit en intervalle)      1,5 ms → 1,5 ms     ×1
#
#  LES DEUX VERROUS QUI RESTENT, chiffrés :
#   1. PLANIFICATEUR — `STARTS WITH p` n'est JAMAIS traduit en intervalle. La
#      machinerie existe pourtant (`try_plan_filter_with_range_index` →
#      `extract_between_predicate` gère `>= a AND < b`), et l'équivalent écrit à
#      la main donne la mesure ci-dessus (×139). Borné.
#   2. SUBSTRAT — la clef d'index est TRONQUÉE À 11 OCTETS (`LARGEUR_CLE`,
#      `encoder_cle_ordonnee`). La troncature est SÛRE (mêmes bornes des deux
#      côtés) mais COLLISIONNE : tous les `qualified_name` de ce graphe
#      partagent leurs 11 premiers octets (`home.trivie`), donc l'index rend
#      « tous les candidats » et la requête retombe sur un balayage — mesuré
#      4,3 ms pour un intervalle d'UNE ligne sur des chemins longs, contre
#      0,028 ms sur des clefs courtes. C'est le verrou des chemins de fichiers,
#      URLs et noms qualifiés. Élargir la clef = changement de FORMAT de série
#      (persistance, migration) dans `props_index.rs`.
#
# ROUND 3 — CE QUI A ÉTÉ CORRIGÉ, MESURÉ, PUIS GARDÉ OU RETIRÉ :
#
#  · TÉMOINS RÉPARÉS. `id(n) < 0` est désormais REPLIÉ par le planificateur (il
#    prouve le prédicat impossible) : la sonde ne balayait plus rien et rendait
#    le banc menteur. Remplacée par `id(n) >= 0`, qui balaie vraiment.
#  · DIAGNOSTIC DU RÉSIDU « nœuds riches ». Le MÊME parcours coûte ~200 ns par
#    cellule sur une arène de 9,6 Mo (en cache) et 530 à 1 077 ns sur celle du
#    snapshot (566 Mo) : c'est de la MÉMOIRE, pas du CPU — et le nombre de
#    propriétés n'ajoute qu'un facteur ~1,6 (File 1 077 → Function 663 ns).
#  · AMORÇAGE `_mm_prefetch` des unités : ÉCRIT, MESURÉ, RETIRÉ. A/B dans le
#    même binaire, temps CPU → ×1,00 sur File et Function, ×0,96 sur le
#    balayage complet. Le parallélisme mémoire entre cellules existe déjà.
#  · AIGUILLES GROUPÉES : `CONTAINS 'a' OR CONTAINS 'b' OR …` demandait N
#    traversées de chaîne pour la MÊME propriété, alors que la lecture de
#    valeurs en faisait une. Une seule traversée, N verdicts :
#       4 termes + 1 cellule   270 ms → 173 ms   (347 970 cellules)
#  · RÉSULTAT FINAL en régime stationnaire (médianes) :
#       balayage nu 78 ns/ligne · +1 cellule absente 208 · +1 cellule présente 252
#       `name CONTAINS 'iam'` (ta requête 1)  66,7 ms  · 192 ns/ligne
#       `STARTS WITH` préfixe (ta requête 3)  40,7 ms
#
# A/B DU ROUND 2 (temps CPU, mêmes binaires/même charge — métrique insensible
# à la contention des autres sessions) :
#
#   supprimer la COPIE DE PAGE HEAP (4 Ko par cellule lue, `read_heap`) :
#     micro-store 2 000 cellules 100 % présentes  1,1 ms → 0,64-0,89 ms  ×1,3-1,7
#   puis VERDICTS TEXTE sans allocation (voir `GraphPredicatTexte`), avec choix
#   ADAPTATIF par densité mesurée (`DENSITE_MIN_VERDICTS`) :
#     micro 100 % présentes          ×1,50
#     qualified_name 100 % (snapshot) ×1,33
#     name 15 % (snapshot)            ×1,34
#     STARTS WITH préfixe (snapshot)  ×1,32
#   Sans l'adaptatif, la propriété CREUSE perdait 16 % (prouver NULL oblige à
#   traverser toute la chaîne) : c'est la densité mesurée qui tranche.
#
#   RÉSULTAT DE BOUT EN BOUT, régime stationnaire, snapshot figé :
#     `name CONTAINS 'iam'`  (ta requête 1)  113-140 ms → 66,9-68,7 ms
#     `STARTS WITH` préfixe  (ta requête 3)  ~95 ms     → 42,6 ms
#
# A/B ENTRELACÉ (2026-09-12, deux binaires, 3 tours alternés, temps CPU) :
#   balayage + 1 cellule   avant 249-254 ms → après 231-236 ms   ≈ 5 %
#   nœuds File (57 propriétés)  inchangé
#   → le décodage des entrées n'était PAS le coût : sauter les valeurs non
#     cherchées (balayage clef+taille) ne rapporte que ~5 %. Ce qui reste est
#     l'ACCÈS aux unités (chaîne de pages dans une arène de 566 Mo) et le
#     nombre de chaînons traversés — nœuds riches : 2,6-3,2 µs/cellule contre
#     ~0,7 µs sur un nœud moyen. La réponse y est structurelle (annuaire de
#     clefs par unité, ou présence par nœud), pas un facteur constant de plus.
#
# RÉSULTATS EN MODE « À FROID » (un processus par mesure, minima sur 5 tours) :
#
#   mesure                          avant      après     gain
#   A. balayage nu 0 cellule        34,5 ms    30,2 ms    (témoin : inchangé)
#   B. balayage + 1 cellule        762,9 ms   368,0 ms    2,1×  (2 192 → 1 127 ns/ligne)
#   C. balayage + 4 cellules      1 990 ms    496,7 ms    4,0×
#   D. name CONTAINS 'iam'        1 070 ms    333,8 ms    3,2×
#   E. 4 termes auth/token        1 280 ms    242,1 ms    5,3×
#   F. STARTS WITH préfixe         364,7 ms   168,5 ms    2,2×
#   G. arêtes, 2 cellules         4 510 ms   3 000 ms     1,5×
#   J. intervalle (scan de zone)   218,9 ms   104,7 ms    2,1×
#
# Ce qui a été payé, dans l'ordre de grandeur mesuré :
#   1. la lecture PAR LOT du filtre — une lecture par chunk et par propriété, au
#      lieu d'une par ligne ET par occurrence de prédicat (le `OR` de 4 termes
#      relisait la même propriété 4 fois) ;
#   2. le repli d'ÉPOQUE borné : à l'époque COURANTE et sans transaction, il
#      hydrate TOUT le nœud pour rendre `None` — 3,3 µs/ligne sur une propriété
#      absente, qui est le cas le plus fréquent d'un filtre
#      (« balayage + 1 cellule absente » : 1 280 ms → 70 ms).
#
# Ce qui n'est PAS résolu, et qu'aucun facteur constant ne résoudra : les
# 347 970 lignes balayées. Un balayage ne descend pas sous ~100 ns/ligne, donc
# une requête sélective ne peut pas être en ns SANS INDEX (index de préfixe /
# n-grammes, élagage par type d'arête). Voir docs/PERF-LISTES-2026-09-07.md.
#
# USAGE. `scripts/bench-filtre-propriete.sh [STORE]` — défaut : le banc figé
# `~/bench-stores/runtime-dev-snapshot-20260909.obrain` (aucun écrivain
# concurrent, donc reproductible). C'est un banc de LECTURE : il ouvre le
# store en écriture (verrou exclusif), il ne peut donc PAS viser runtime-dev
# tant que le harness le tient.
#
# SORTIE. Une ligne par mesure : min / médiane sur N tour, et le rapport
# cellules/ligne implicite quand il est calculable. `OBRAIN_BIN` et `TOURS`
# surchargent le binaire et le nombre de tours.

set -uo pipefail

STORE="${1:-$HOME/bench-stores/runtime-dev-snapshot-20260909.obrain}"
BIN="${OBRAIN_BIN:-$(dirname "$0")/../target/release/obrain}"
TOURS="${TOURS:-3}"

if [[ ! -x "$BIN" ]]; then
    echo "binaire absent : $BIN" >&2
    exit 2
fi
if [[ ! -d "$STORE" ]]; then
    echo "store absent : $STORE" >&2
    exit 2
fi

BIN=$(readlink -f "$BIN")
# Le nombre de nœuds se DEMANDE au moteur : `substrate.nodes/16` est la
# CAPACITÉ allouée (1 M ici), pas la population (347 970) — un diviseur faux de
# 2,9× rendrait tous les ns/ligne faux du même facteur.
NOEUD_MIN=$("$BIN" query "$STORE" "MATCH (n) RETURN count(n)" --format json 2>/dev/null \
            | tr -dc '0-9')
NOEUD_MIN=${NOEUD_MIN:-1}

# Le CLI rend « 1 row (205us) » aussi bien que « (1.23ms) » : les deux unités.
en_ms() {
    case "$1" in
        *us) awk -v v="${1%us}" 'BEGIN{print v/1000}' ;;
        *ms) printf '%s' "${1%ms}" ;;
        *s)  awk -v v="${1%s}"  'BEGIN{print v*1000}' ;;
        *)   printf '0' ;;
    esac
}

mesure() {
    local nom="$1" q="$2"
    local -a t=()
    for _ in $(seq "$TOURS"); do
        local out
        out=$("$BIN" query "$STORE" "$q" --timing --format table 2>/dev/null \
              | grep -oE '\([0-9.]+(us|ms|s)\)' | tail -1 | tr -d '()')
        [[ -z "$out" ]] && continue
        t+=("$(en_ms "$out")")
    done
    if [[ ${#t[@]} -eq 0 ]]; then
        printf '%-34s %s\n' "$nom" "ÉCHEC"
        return
    fi
    printf '%s\n' "${t[@]}" | sort -g | awk -v n="$nom" -v lignes="$NOEUD_MIN" '
        { v[NR]=$1; s+=$1 }
        END {
            med = (NR%2) ? v[(NR+1)/2] : (v[NR/2]+v[NR/2+1])/2
            ns = med*1e6/lignes
            printf "%-34s min %9.2f ms   médiane %9.2f ms   %6.0f ns/ligne\n", n, v[1], med, ns
        }'
}

# ── RÉGIME STATIONNAIRE ────────────────────────────────────────────────
#
# POURQUOI CE SECOND MODE, ET POURQUOI C'EST CELUI QUI COMPTE.
#
# Le mode ci-dessus lance UN PROCESSUS par mesure : chaque requête paie donc
# l'ouverture du store, la mise en place des mmap et le FAUTAGE DES PAGES
# touchées. Mesuré sur ce snapshot (`/usr/bin/time -v`, 2026-09-12) :
# 36 426 défauts de page mineurs, 0 majeur, pour une requête qui lit `n.name`
# sur 347 970 nœuds — soit 0,28 s de temps SYSTÈME pour 0,28 s de temps
# utilisateur. La moitié du coût d'une mesure du mode « à froid » n'est donc pas
# le moteur, c'est le noyau.
#
# Or le chemin réel de l'utilisateur (`obrain_query` du harness) passe par un
# bridge `obrain bridge` PERSISTANT : le store reste ouvert, les mmap restent en
# place, les défauts de page sont payés une fois puis jamais. Ce mode-ci mesure
# ça : N exécutions de la même requête DANS UN SEUL processus (`obrain shell`),
# et on ne retient que la DERNIÈRE — les précédentes ont chauffé les tables de
# pages.
#
# Écart mesuré sur la mesure B (balayage + 1 cellule) : 410 ms à froid,
# 207 ms en régime stationnaire.
mesure_stationnaire() {
    local nom="$1" q="$2" tours="${3:-4}"
    local corps
    corps=$(for _ in $(seq "$tours"); do printf '%s;\n' "$q"; done)
    local derniere
    derniere=$(printf '%s' "$corps" | "$BIN" shell "$STORE" --timing --format table 2>/dev/null \
               | grep -oE '\([0-9.]+(us|ms|s)\)' | tail -1 | tr -d '()')
    if [[ -z "$derniere" ]]; then
        printf '%-34s %s\n' "$nom" "ÉCHEC"
        return
    fi
    awk -v n="$nom" -v ms="$(en_ms "$derniere")" -v lignes="$NOEUD_MIN" '
        BEGIN { printf "%-34s %9.2f ms   %6.0f ns/ligne\n", n, ms, ms*1e6/lignes }'
}


echo "store  : $STORE"
echo "nœuds  : $NOEUD_MIN (population réelle)"
echo "binaire: $BIN"
echo "tours  : $TOURS"
echo

echo "-- témoins (le balayage, sans et avec cellules) --"
mesure "A. balayage nu 0 cellule"   "MATCH (n) WHERE id(n) >= 0 RETURN 1"
mesure "B. balayage + 1 cellule"    "MATCH (n) WHERE id(n) >= 0 AND n.name CONTAINS 'zzqq' RETURN 1"
mesure "C. balayage + 4 cellules"   "MATCH (n) WHERE id(n) >= 0 AND (n.name CONTAINS 'zq1' OR n.name CONTAINS 'zq2' OR n.name CONTAINS 'zq3' OR n.name CONTAINS 'zq4') RETURN 1"
echo
echo "-- requêtes réelles (celles du log MCP du 2026-09-12) --"
mesure "D. name CONTAINS 'iam'"     "MATCH (n) WHERE n.name CONTAINS 'iam' RETURN n.qualified_name, labels(n) LIMIT 30"
mesure "E. 4 termes auth/token"     "MATCH (n) WHERE n.name CONTAINS 'auth' OR n.name CONTAINS 'Auth' OR n.name CONTAINS 'token' OR n.name CONTAINS 'Token' RETURN n.qualified_name, labels(n) LIMIT 30"
mesure "F. STARTS WITH préfixe"     "MATCH (n) WHERE n.qualified_name STARTS WITH 'home.triviere.projects.banc_obrain.crates.obrain-iam' RETURN n.qualified_name, labels(n) LIMIT 60"
mesure "G. arêtes, 2 cellules"      "MATCH (a)-[r]->(b) WHERE a.qualified_name CONTAINS 'obrain-iam' AND NOT b.qualified_name CONTAINS 'obrain-iam' RETURN a.qualified_name, type(r), b.qualified_name LIMIT 40"
echo
echo "-- références de ce qui marche déjà --"
mesure "H. compteur (coût fixe)"    "MATCH (n) RETURN count(n)"
mesure "I. égalité indexée"         "MATCH (n) WHERE n.qualified_name = 'home.triviere.projects.banc_obrain.workspace.dependencies.obrain-iam' RETURN n.qualified_name LIMIT 5"
mesure "J. intervalle (scan zone)"  "MATCH (n) WHERE n.qualified_name >= 'home.triviere.projects.banc_obrain.crates.obrain-iam' AND n.qualified_name < 'home.triviere.projects.banc_obrain.crates.obrain-iamz' RETURN n.qualified_name LIMIT 60"
echo
echo "-- RÉGIME STATIONNAIRE (une seule ouverture, N exécutions, dernière retenue) --"
echo "   c'est le profil du bridge persistant de l'utilisateur, pas celui du CLI"
mesure_stationnaire "K. balayage nu 0 cellule"  "MATCH (n) WHERE id(n) >= 0 RETURN 1"
mesure_stationnaire "L. +1 cellule absente"     "MATCH (n) WHERE id(n) >= 0 AND n.zzqqabsent CONTAINS 'x' RETURN 1"
mesure_stationnaire "M. +1 cellule présente"    "MATCH (n) WHERE id(n) >= 0 AND n.name CONTAINS 'zzqq' RETURN 1"
mesure_stationnaire "N. 4 termes OR"            "MATCH (n) WHERE id(n) >= 0 AND (n.name CONTAINS 'a1' OR n.name CONTAINS 'a2' OR n.name CONTAINS 'a3' OR n.name CONTAINS 'a4') RETURN 1"
mesure_stationnaire "O. name CONTAINS 'iam'"    "MATCH (n) WHERE n.name CONTAINS 'iam' RETURN n.qualified_name, labels(n) LIMIT 30"
mesure_stationnaire "P. arêtes, 2 cellules"     "MATCH (a)-[r]->(b) WHERE a.qualified_name CONTAINS 'obrain-iam' AND NOT b.qualified_name CONTAINS 'obrain-iam' RETURN a.qualified_name, type(r), b.qualified_name LIMIT 40"

