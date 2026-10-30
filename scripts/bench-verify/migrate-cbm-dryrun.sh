#!/usr/bin/env bash
# =============================================================================
# migrate-cbm-dryrun.sh — Dry-run de migration des index de code legacy
# (serveur C codebase-memory-mcp : bases SQLite ~/.cache/codebase-memory-mcp/*.db)
# vers des stores substrate obrain produits par RE-INDEXATION NATIVE
# (obrain-indexer --example persist). SANS déploiement, SANS suppression.
#
# PÉRIMÈTRE / SÉCURITÉ :
#   * Bases legacy ouvertes en LECTURE SEULE (sqlite mode=ro) : rien n'est
#     modifié ni supprimé (pas de checkpoint WAL, pas de .db écrit).
#   * Stores écrits UNIQUEMENT dans $MIGRATE_CBM_OUT (défaut /tmp/migrate-cbm/out),
#     jamais dans les arbres de travail, ~/.local/share/obrain-harness ni harness.
#   * Aucune modification de code Rust : binaire target/release/examples/persist
#     réutilisé tel quel (fraîcheur assurée par le gardien bench-verify/guard.sh).
#     Reconstruction optionnelle : MIGRATE_CBM_BUILD=1 (cargo build, pas de modif
#     de source — à éviter tant qu'un autre agent travaille dans obrain-indexer).
#
# Usage :
#   scripts/bench-verify/migrate-cbm-dryrun.sh                 # dry-run complet
#   scripts/bench-verify/migrate-cbm-dryrun.sh --verify <store> [store…]
#                                                              # vérif bridge seule
# Env :
#   MIGRATE_CBM_CACHE     répertoire des bases (défaut ~/.cache/codebase-memory-mcp)
#   MIGRATE_CBM_OUT       répertoire des stores (défaut /tmp/migrate-cbm/out)
#   MIGRATE_CBM_TIMEOUT   timeout par projet, s (défaut 150)
#   MIGRATE_CBM_GLOBAL    budget total du dry-run, s (défaut 300)
#   MIGRATE_CBM_ORDER     tri des tentatives : size (défaut, arbre croissant) | alpha
#   MIGRATE_CBM_BUILD     1 => cargo build --release -p obrain-indexer --example persist
#   MIGRATE_CBM_PERSIST   chemin du binaire persist (défaut target/release/examples/persist)
#
# Sortie : tableau projet | racine | nœuds persistés | nœuds C (approx)
#          | store Mo | durée | note, puis « RÉSUMÉ : N projets migrés (dry-run)
#          sur M listés ; stores dans <OUT> ».
# =============================================================================
set -u

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CACHE="${MIGRATE_CBM_CACHE:-$HOME/.cache/codebase-memory-mcp}"
OUT="${MIGRATE_CBM_OUT:-/tmp/migrate-cbm/out}"
PER_TIMEOUT="${MIGRATE_CBM_TIMEOUT:-150}"
GLOBAL_CAP="${MIGRATE_CBM_GLOBAL:-300}"
ORDER="${MIGRATE_CBM_ORDER:-size}"
PERSIST="${MIGRATE_CBM_PERSIST:-$ROOT/target/release/examples/persist}"
BRIDGE="${MIGRATE_CBM_BRIDGE:-$ROOT/target/release/obrain}"

# --- mode --verify : vérifier des stores produits via le bridge MCP ----------
if [ "${1:-}" = "--verify" ]; then
    shift
    [ $# -ge 1 ] || { echo "usage: $0 --verify <store.obrain> [store…]" >&2; exit 2; }
    for STORE in "$@"; do
        [ -d "$STORE" ] || { echo "store introuvable : $STORE" >&2; exit 2; }
        echo "== store $(basename "$STORE") =="
        for Q in 'MATCH (n) RETURN count(n) AS n' 'MATCH (n:Project) RETURN count(n) AS n' 'MATCH (n:Function) RETURN count(n) AS n'; do
            printf '%s\n' "{\"tool\":\"query_graph\",\"args\":{\"query\":\"$Q\"}}" \
              | env OBRAIN_STORE="$STORE" "$BRIDGE" bridge 2>/dev/null \
              | python3 -c 'import sys,json
try:
    d=json.loads(sys.stdin.read())
    rows=(d.get("structuredContent") or {}).get("rows") or []
    print("  query_graph ->", rows[0][0] if rows else "?")
except Exception as e:
    print("  query_graph -> ECHEC", e)'
        done
        printf '%s\n' '{"tool":"list_projects","args":{}}' \
          | env OBRAIN_STORE="$STORE" "$BRIDGE" bridge 2>/dev/null \
          | python3 -c 'import sys,json
try:
    d=json.loads(sys.stdin.read())
    ps=(d.get("structuredContent") or {}).get("projects") or []
    print("  list_projects ->", [p.get("name") for p in ps])
except Exception as e:
    print("  list_projects -> ECHEC", e)'
    done
    exit 0
fi

[ -d "$CACHE" ] || { echo "ÉCHEC : $CACHE introuvable" >&2; exit 1; }
[ -x "$PERSIST" ] || { echo "ÉCHEC : binaire persist absent ($PERSIST) — lancer d'abord : cargo build --release -p obrain-indexer --example persist" >&2; exit 1; }
[ -x "$BRIDGE" ] || { echo "ÉCHEC : binaire obrain absent ($BRIDGE)" >&2; exit 1; }
mkdir -p "$OUT"

# Reconstruction optionnelle (désactivée par défaut : l'autre agent travaille
# dans obrain-indexer ; le binaire est déjà frais via guard.sh).
if [ "${MIGRATE_CBM_BUILD:-0}" = "1" ]; then
    echo "== cargo build --release -p obrain-indexer --example persist =="
    (cd "$ROOT" && cargo build --release -p obrain-indexer --example persist) || exit 1
fi

echo "== dry-run migration C->substrate obrain =="
echo "bases  : $CACHE"
echo "stores : $OUT        (timeout/projet ${PER_TIMEOUT}s, budget global ${GLOBAL_CAP}s, tri=$ORDER)"

T0=$(date +%s)

# ---------------------------------------------------------------------------
# 1) Inventaire des projets legacy (bases *.db, hors _config/-dev/-stage)
#    + comptage « nœuds C (approx) » en lecture seule + tri demandé.
#    Colonnes (séparateur \x1f — jamais tab/space : read collapserait les
#    champs vides) : base | projet | racine | nodes_c | nodes_c_all | raison
# ---------------------------------------------------------------------------
DISCO=$(mktemp)
python3 - "$CACHE" "$ORDER" > "$DISCO" <<'PYEOF'
import sqlite3, glob, os, sys
cache, order = sys.argv[1], sys.argv[2]
# Labels de télémétrie runtime (fusionnés dans certaines bases C) : exclus du
# compte « code » pour comparer comparable-à-comparable avec persist.
TEL = {"Message", "Step", "ToolCall", "ToolResult", "Session", "Workspace",
       "Account", "Skill", "Plan", "Goal", "Engram", "FileSpan"}

def count_nodes(con, proj):
    cur = con.cursor()
    try:
        allc = cur.execute("SELECT COUNT(*) FROM nodes WHERE project=?",
                           (proj,)).fetchone()[0]
    except Exception:
        return None, None
    codec = allc
    tel = 0
    try:
        for lab, c in cur.execute(
                "SELECT label, COUNT(*) FROM nodes WHERE project=? GROUP BY label",
                (proj,)):
            if lab in TEL:
                tel += c
    except Exception:
        pass
    codec = allc - tel
    return codec, allc

rows = []
for db in sorted(glob.glob(cache + "/*.db")):
    base = os.path.basename(db)
    if base == "_config.db" or "-dev" in base or "-stage" in base:
        continue
    try:
        con = sqlite3.connect("file:%s?mode=ro" % db, uri=True, timeout=15)
        cur = con.cursor()
        tabs = [r[0] for r in cur.execute(
            "SELECT name FROM sqlite_master WHERE type='table'")]
        if "projects" not in tabs or "nodes" not in tabs:
            rows.append((base, "", "", "", "", "schéma inattendu")); con.close(); continue
        projs = cur.execute(
            "SELECT name, root_path FROM projects WHERE root_path != ''").fetchall()
        if not projs:
            rows.append((base, "", "", "", "", "racine vide")); con.close(); continue
        proj, root = projs[0]
        if not os.path.isdir(root):
            rows.append((base, proj, root, "", "", "racine absente")); con.close(); continue
        codec, allc = count_nodes(con, proj)
        rows.append((base, proj, root,
                     str(codec) if codec is not None else "?",
                     str(allc) if allc is not None else "?", ""))
        con.close()
    except Exception as e:
        rows.append((base, "", "", "", "", "db illisible: %s" % repr(e)[:60]))

# ordre : size (arbres croissants, tentatives d'abord) | alpha (nom de base)
if order == "size":
    rows.sort(key=lambda r: (r[5] != "",  # tentables d'abord
                             0 if r[5] == "" else 1,
                             int(r[3]) if r[3].isdigit() else 10**15,
                             r[0]))
SEP = "\x1f"
for r in rows:
    print(SEP.join(r))
PYEOF
[ -s "$DISCO" ] || { echo "ÉCHEC : aucun projet découvert" >&2; rm -f "$DISCO"; exit 1; }

# Lecture des lignes découvertes dans des tableaux bash
declare -a BASE PROJ ROOT NC NCALL REASON
M=0
while IFS=$'\x1f' read -r b p r nc nca rs; do
    BASE[$M]="$b"; PROJ[$M]="$p"; ROOT[$M]="$r"
    NC[$M]="$nc"; NCALL[$M]="$nca"; REASON[$M]="$rs"
    M=$((M+1))
done < "$DISCO"
rm -f "$DISCO"

# ---------------------------------------------------------------------------
# 2) Tentatives : tri interne (du -sk de l'arbre, excl. .git/target/node_modules),
#    dédoublonnage des racines déjà indexées, timeout par projet, budget global.
# ---------------------------------------------------------------------------
declare -a STATUS NOTE DUR NODESP MB
declare -A ROOT_DONE            # root -> store produit
ORDER_IDX=()
for i in $(seq 0 $((M-1))); do
    STATUS[$i]="non tenté"; NOTE[$i]="${REASON[$i]:-}"; DUR[$i]="-"; NODESP[$i]="-"; MB[$i]="-"
    if [ -n "${REASON[$i]}" ]; then
        continue
    fi
    # du de l'arbre (proxy du coût persist) — exclut les gros non-code
    TKB=$(du -sk --exclude=.git --exclude=target --exclude=node_modules "${ROOT[$i]}" 2>/dev/null | awk '{print $1}')
    ORDER_IDX+=("$i|${TKB:-0}")
done
# tri par taille d'arbre croissante (ou ordre alpha déjà fourni par python)
if [ "$ORDER" = "size" ]; then
    ORDER_IDX=($(printf '%s\n' "${ORDER_IDX[@]}" | sort -t'|' -k2 -n))
fi

N_OK=0
CAP_HIT=0
for entry in "${ORDER_IDX[@]}"; do
    i=${entry%%|*}
    b="${BASE[$i]}"; p="${PROJ[$i]}"; r="${ROOT[$i]}"
    STORE="$OUT/${b%.db}.obrain"

    # budget global restant
    NOW=$(date +%s); REM=$((GLOBAL_CAP - (NOW - T0)))
    if [ "$REM" -le 0 ]; then CAP_HIT=1; break; fi

    if [ -n "${ROOT_DONE[$r]:-}" ]; then
        STATUS[$i]="doublon"
        NOTE[$i]="racine déjà indexée → store partagé ${ROOT_DONE[$r]}"
        continue
    fi
    # filet de sécurité : la racine doit exister au moment de la tentative
    if [ ! -d "$r" ]; then
        STATUS[$i]="ignoré"
        NOTE[$i]="racine absente (vérifiée à la tentative)"
        continue
    fi
    # étiquette taille d'arbre
    TKB=$(du -sk --exclude=.git --exclude=target --exclude=node_modules "$r" 2>/dev/null | awk '{print $1}')
    TSZ=$(numfmt --to=iec "${TKB:-0}K" 2>/dev/null || echo "${TKB:-0}K")

    echo "── $b : $p  (arbre $TSZ, racine $r)"
    rm -rf "$STORE"
    LOG=$(mktemp)
    ET=$PER_TIMEOUT; [ "$REM" -lt "$ET" ] && ET=$REM
    S=$(date +%s%N)
    timeout "$ET" "$PERSIST" "$r" "$STORE" > "$LOG" 2>&1
    RC=$?
    E=$(date +%s%N)
    SECS=$(awk -v a="$S" -v b="$E" 'BEGIN{printf "%.1f", (b-a)/1e9}')
    grep -E '^(index|noeuds|persist|total|repo|out)' "$LOG" | sed 's/^/    /'

    if [ "$RC" -eq 0 ] && [ -d "$STORE" ]; then
        NODES=$(grep -oE 'noeuds *: *[0-9]+' "$LOG" | head -1 | grep -oE '[0-9]+$')
        NODESP[$i]="${NODES:-?}"
        MBK=$(du -sk "$STORE" 2>/dev/null | awk '{print $1}')
        MB[$i]=$(awk -v k="${MBK:-0}" 'BEGIN{printf "%.1f", k/1024}')
        DUR[$i]="$SECS"
        STATUS[$i]="ok"
        ROOT_DONE["$r"]="$(basename "$STORE")"
        NOTE[$i]="arbre $TSZ"
        N_OK=$((N_OK+1))
        echo "    → store $(basename "$STORE") : ${NODESP[$i]} nœuds, ${MB[$i]} Mo, ${SECS}s"
    else
        rm -rf "$STORE"
        STATUS[$i]="échec"
        if [ "$RC" -eq 124 ]; then
            NOTE[$i]="timeout > ${ET}s (arbre $TSZ)"
        else
            NOTE[$i]="persist rc=$RC (arbre $TSZ)"
        fi
        DUR[$i]="$SECS"
        echo "    → échec (rc=$RC, ${SECS}s) : ${NOTE[$i]}"
    fi
    rm -f "$LOG"
done
if [ "$CAP_HIT" -eq 1 ]; then
    for i in "${ORDER_IDX[@]}"; do
        i=${i%%|*}
        [ "${STATUS[$i]}" = "non tenté" ] && NOTE[$i]="cap global (${GLOBAL_CAP}s) atteint"
    done
fi
WALL=$(( $(date +%s) - T0 ))

# ---------------------------------------------------------------------------
# 3) Tableau final + RÉSUMÉ
# ---------------------------------------------------------------------------
pr() { printf "$@"; }
echo
echo "projet                                     | racine                                   | persistés | C(approx) | store Mo | durée(s) | note"
echo "-------------------------------------------|-----------------------------------------|-----------|-----------|----------|----------|------------------------------------"
trunc() { local s="$1" n="$2"; if [ "${#s}" -le "$n" ]; then echo "$s"; else echo "${s:0:$((n/2-1))}…${s: -$((n/2-1))}"; fi; }
for i in $(seq 0 $((M-1))); do
    pcol="$(trunc "${PROJ[$i]}" 43)"
    rcol="$(trunc "${ROOT[$i]}" 41)"
    cc="${NC[$i]}"; [ "$cc" != "${NCALL[$i]}" ] && [ -n "${NCALL[$i]}" ] && cc="$cc*"
    [ -z "$cc" ] && cc="-"
    pr "%-43s | %-41s | %9s | %9s | %8s | %8s | %s\n" \
       "$pcol" "$rcol" "${NODESP[$i]}" "$cc" "${MB[$i]}" "${DUR[$i]}" "${NOTE[$i]}"
done
echo "-------------------------------------------|-----------------------------------------|-----------|-----------|----------|----------|------------------------------------"
echo "(* = hors télémétrie runtime fusionnée dans la base C)"
echo
echo "RÉSUMÉ : $N_OK projets migrés (dry-run) sur $M listés ; stores dans $OUT ; durée murale ${WALL}s"
