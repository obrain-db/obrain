#!/usr/bin/env bash
# Banc par le SERVEUR viz — le store VIVANT, tel que tu le consultes.
#
# POURQUOI CE BANC EXISTE, À CÔTÉ DE `bench-filtre-propriete.sh`.
#
#   · `bench-filtre-propriete.sh` ouvre le store par le CLI, donc en verrou
#     EXCLUSIF : il ne peut PAS viser `runtime-dev`, que le harness tient en
#     permanence. Il mesure sur la fixture figée (566 Mo d'arène, aucun écrivain).
#   · Le store vivant est un autre régime : arène de 2,1 Go (plus de latence
#     mémoire), ÉCRIVAIN CONCURRENT (l'indexeur du harness écrit en continu :
#     churn de pages, contention sur les verrous de zone), et chaque appel est
#     une exécution FRAÎCHE. C'est pourtant celui-là que tu regardes.
#   · Ce banc-ci passe par le serveur (lecture seule), donc il atteint le store
#     vivant malgré le verrou, et il mesure EXACTEMENT la grandeur que ton
#     interface affiche : `execution_time_ms`, le temps moteur seul.
#
# LE PIÈGE DU CACHE, ET COMMENT IL EST CONTOURNÉ ICI.
# Le serveur garde un cache durable des requêtes lecture, clé = (store, texte,
# mtime). Une reprise après écriture est donc invalide, mais relancer la MÊME
# requête juste après rend un corps en cache : le temps MURAL tombe à ~0,6 ms et
# le `execution_time_ms` affiché est celui d'une exécution ANTÉRIEURE — on
# croirait mesurer le moteur alors qu'on lit un souvenir. Chaque tour ajoute donc
# une ESPACE FINALE au texte (clé différente, analyse identique) : toute mesure
# imprimée est une exécution réelle.
#
# RÉSULTATS MESURÉS (2026-09-13, serveur 8432, runtime-dev vivant :
# 408 832 nœuds / 1 256 548 arêtes, minima sur 3 exécutions fraîches) :
#
#   requête du log MCP du 2026-09-12            log       actuel     gain
#   1. name/qualified_name CONTAINS iam         487 ms    100,4 ms   ×4,9
#   2. 4 termes auth/token                      607 ms     80,4 ms   ×7,5
#   3. STARTS WITH obrain-iam                   101 ms     48,6 ms   ×2,1
#   4. File + CONTAINS                           34 ms      7,8 ms   ×4,4
#   5. STARTS WITH src + types (reconstruite)   241 ms    179,0 ms   ×1,3
#   6. arêtes, 2 cellules                      3512 ms    805,5 ms   ×4,4
#   7. STARTS WITH src + Function/Method        148 ms     97,8 ms   ×1,5
#   8. arêtes TESTS, 0 ligne                   2013 ms    117,5 ms   ×17,1
#   9. égalité indexée (repère)                    —       0,07 ms
#  10. intervalle sur qualified_name               —      52,8 ms   (BALAYAGE : clef
#                                                                    d'index tronquée
#                                                                    à 11 octets, tous
#                                                                    les chemins
#                                                                    collisionnent)
#  11. compteur (coût fixe du serveur)             —       0,02 ms
#
# À LIRE AVANT DE COMPARER : le PREMIER appel d'une requête dans un processus
# froid paie le fautage des pages du mmap. Mesuré sur la requête 1 : ~299 ms au
# premier appel, 100 ms une fois le processus chaud (c'est pourquoi le banc fait
# plusieurs tours et retient le minimum). Un chiffre isolé pris juste après un
# redémarrage du serveur est donc pessimiste d'un facteur ~3 sur cette requête.
#
# LES DEUX REQUÊTES QUI NE GAGNENT PRESQUE RIEN (5 et 7) sont celles qui mêlent
# LABEL et PRÉFIXE : elles attendent le chemin index (`STARTS WITH` → intervalle)
# et, pour ces chemins longs, l'élargissement de la clef d'index.
#
# USAGE. `scripts/bench-serveur.sh [URL] [STORE] [TOURS]`
#   défaut : http://192.168.1.103:8432 et le store runtime-dev.

set -uo pipefail

URL="${1:-http://192.168.1.103:8432}"
STORE="${2:-$HOME/.local/share/obrain-harness/data/runtime-dev.obrain}"
TOURS="${3:-3}"

if ! curl -s -m 10 -o /dev/null "$URL/api/viz/status"; then
    echo "serveur injoignable : $URL" >&2
    exit 2
fi

# Contextualise : un chiffre sans la taille du graphe ne veut rien dire.
INFO=$(curl -s -m 30 "$URL/api/worlds" \
       | python3 -c "
import json,sys
d=json.load(sys.stdin)
for m in d.get('mondes', []):
    if m['path'].rstrip('/') == '$STORE'.rstrip('/'):
        print(m['nodes'], m['edges'], m['label']); break
else:
    print('?','?','(monde inconnu du serveur)')
")
read -r NOEUDS ARETES LABEL <<<"$INFO"
echo "serveur : $URL"
echo "store   : $STORE ($LABEL)"
echo "graphe  : $NOEUDS nœuds, $ARETES arêtes"
echo "tours   : $TOURS (chaque tour est une exécution FRAÎCHE)"
echo

# Rejoue une requête et rend « min médiane lignes » du temps MOTEUR, en ms.
mesure() {
    local nom="$1" q="$2" attendu="${3:-}"
    local -a t=()
    local lignes=""
    for i in $(seq "$TOURS"); do
        # L'espace finale change la CLÉ de cache, pas l'analyse (voir l'en-tête).
        local texte="$q "
        for _ in $(seq "$i"); do texte="$texte "; done
        local out
        out=$(curl -s -m 600 -G "$URL/api/query" \
              --data-urlencode "store=$STORE" --data-urlencode "query=$texte")
        local ms
        ms=$(printf '%s' "$out" | python3 -c "
import json,sys
try:
    d=json.load(sys.stdin)
except Exception:
    print(''); raise SystemExit
print(d.get('execution_time_ms',''))" 2>/dev/null)
        local n
        n=$(printf '%s' "$out" | python3 -c "
import json,sys
try:
    d=json.load(sys.stdin)
except Exception:
    print('?'); raise SystemExit
print(len(d.get('rows',[])) if 'rows' in d else 'err')" 2>/dev/null)
        [[ -n "$ms" ]] && t+=("$ms")
        lignes="$n"
    done
    if [[ ${#t[@]} -eq 0 ]]; then
        printf '%-46s %s\n' "$nom" "ÉCHEC"
        return
    fi
    printf '%s\n' "${t[@]}" | sort -g | awk -v n="$nom" -v l="$lignes" -v a="$attendu" -v tot="$NOEUDS" '
        { v[NR]=$1 }
        END {
            med = (NR%2) ? v[(NR+1)/2] : (v[NR/2]+v[NR/2+1])/2
            gain = (a != "" && a+0 > 0 && v[1] > 0) ? sprintf("×%.1f", (a+0)/v[1]) : ""
            printf "%-46s min %9.2f ms   médiane %9.2f ms   %6.0f ns/neud   lignes %-4s %s\n", n, v[1], med, v[1]*1e6/tot, l, (a != "" ? "log " a " ms → " gain : "")
        }'
}

echo "-- tes requêtes du log MCP (2026-09-12), rejouées sur le moteur actuel --"
mesure "1. name/qualified_name CONTAINS iam" \
  "MATCH (n) WHERE n.name CONTAINS 'iam' OR n.name CONTAINS 'IAM' OR n.qualified_name CONTAINS 'iam' RETURN n.qualified_name, labels(n) LIMIT 30" 487
mesure "2. 4 termes auth/token" \
  "MATCH (n) WHERE n.name CONTAINS 'auth' OR n.name CONTAINS 'Auth' OR n.name CONTAINS 'token' OR n.name CONTAINS 'Token' RETURN n.qualified_name, labels(n) LIMIT 30" 607
mesure "3. STARTS WITH obrain-iam" \
  "MATCH (n) WHERE n.qualified_name STARTS WITH 'home.triviere.projects.banc_obrain.crates.obrain-iam' RETURN n.qualified_name, labels(n) LIMIT 60" 101
mesure "4. File + CONTAINS" \
  "MATCH (f:File) WHERE f.qualified_name CONTAINS 'obrain-iam' RETURN f.qualified_name LIMIT 40" 34
mesure "5. STARTS WITH src + types (reconstruite)" \
  "MATCH (n) WHERE n.qualified_name STARTS WITH 'home.triviere.projects.banc_obrain.crates.obrain-iam.src' AND (n:Module OR n:Struct OR n:Enum OR n:Trait OR n:Class OR n:Interface OR n:Type) RETURN n.qualified_name LIMIT 40" 241
mesure "6. arêtes, 2 cellules" \
  "MATCH (a)-[r]->(b) WHERE a.qualified_name CONTAINS 'obrain-iam' AND NOT b.qualified_name CONTAINS 'obrain-iam' RETURN a.qualified_name, type(r), b.qualified_name LIMIT 40" 3512
mesure "7. STARTS WITH src + Function/Method (reconstruite)" \
  "MATCH (n) WHERE n.qualified_name STARTS WITH 'home.triviere.projects.banc_obrain.crates.obrain-iam.src' AND (n:Function OR n:Method) RETURN n.qualified_name LIMIT 80" 148
mesure "8. arêtes TESTS, 0 ligne" \
  "MATCH (a)-[r:TESTS]->(b) WHERE a.qualified_name CONTAINS 'obrain-iam' RETURN a.qualified_name, b.qualified_name LIMIT 30" 2013
echo
echo "-- repères (ce qui marche déjà par index) --"
mesure "9. égalité indexée (qualified_name)" \
  "MATCH (n) WHERE n.qualified_name = 'home.triviere.projects.banc_obrain.workspace.dependencies.obrain-iam' RETURN n.qualified_name LIMIT 5"
mesure "10. intervalle sur qualified_name" \
  "MATCH (n) WHERE n.qualified_name >= 'home.triviere.projects.banc_obrain.crates.obrain-iam' AND n.qualified_name < 'home.triviere.projects.banc_obrain.crates.obrain-iamz' RETURN n.qualified_name LIMIT 60"
mesure "11. compteur (coût fixe du serveur)" "MATCH (n) RETURN count(n)"
