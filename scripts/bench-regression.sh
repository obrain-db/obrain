#!/usr/bin/env bash
# Full regression harness — lancé avant/après chaque step de T17j+ pour
# détecter toute dérive, pas seulement le périmètre du changement en
# cours.
#
# Couvre 4 axes :
#   1. Lib tests (substrate + engine) — no behavioural regression
#   2. DB open bench (PO/Wiki/Megalaw) — no startup regression
#   3. RAM bench (3 corpora) — no memory regression
#   4. Full PO query bench (26 queries) — no perf regression
#   5. CORPUS RÉEL : latences des requêtes réellement exécutées (seuil 50 ms
#      + ligne de base `scripts/banc-requetes-lentes.baseline`). Cet axe mesure
#      ce que l'axe 4 ne mesure pas : les requêtes du JOURNAL MCP, dans un
#      processus JETABLE (le pire cas), sur un store de production figé.
#   6. CHEMIN DU CHAT : bridge + runtime_search + context_build (enveloppe à
#      seuil — le banc d'origine rapporte sans sanctionner)
#   7. ENDPOINTS `viz serve` : smoke-backend tel quel (il sort 1 en échec)
#   8. Corpus réel (5) et axes 6-7 : joués AUSSI en mode `open-only`
#
# Usage :
#   scripts/bench-regression.sh               # run all
#   scripts/bench-regression.sh open-only     # fast pre-commit check
#   scripts/bench-regression.sh --no-write    # skip writes on existing DBs
#
# Output : sauvegarde chaque log dans /tmp/bench-regression-$(date +%s)/
# + affiche un récap final avec verdicts PASS/FAIL/DIFF.
#
# Gates par défaut (configurable via env) :
#   T17J_GATE_PO_OPEN_MS=20
#   T17J_GATE_WIKI_OPEN_MS=30
#   T17J_GATE_MEGALAW_OPEN_MS=45
#   T17J_GATE_MOST_CONNECTED_MS=30
#   T17J_GATE_COUNT_N_MS=1
#   T17J_GATE_RAM_PO_MB=150
#   T17J_GATE_RAM_MEGALAW_MB=1200
#   T17J_GATE_RAM_WIKI_MB=5000
#
# Axes 6-7 — les deux autres contextes nommés :
#   OBRAIN_CHAT_STORE    store du banc du chat (défaut : le banc figé)
#   OBRAIN_CHAT_SEUIL_MS seuil du chemin du chat (défaut 50)
#   OBRAIN_VIZ_BASE      base du serveur viz (défaut http://192.168.1.103:8432)
#
# Axe 5 — variables :
#   OBRAIN_BANC_STORE   store mesuré (défaut : le banc figé du chantier)
#   OBRAIN_BANC_SEUIL   seuil objectif en ms (défaut 50)
#   OBRAIN_BANC_TOLERANCE  tolérance sur la ligne de base (défaut 1,5)
#
# L'axe 5 est IGNORÉ si le store est absent : le harnais doit rester utilisable
# sur une machine sans les bases — on le dit, on ne le fait pas passer en
# silence.

set -euo pipefail

MODE="${1:-full}"
TS="$(date +%s)"
OUTDIR="/tmp/bench-regression-$TS"
mkdir -p "$OUTDIR"
echo "═══ T17j regression harness — logs : $OUTDIR ═══"

PASS=()
FAIL=()

run_or_fail() {
    local label="$1"
    shift
    local logfile="$OUTDIR/${label// /_}.log"
    echo "→ $label"
    if "$@" > "$logfile" 2>&1; then
        PASS+=("$label")
        tail -1 "$logfile" | sed 's/^/    /'
    else
        # ── LES ÉCHECS CONNUS NE RENDENT PAS LE BANC ROUGE (2026-09-15) ────
        #
        # Constaté : le banc était rouge à cause d'UN test d'une autre session,
        # depuis vingt-cinq tours. Un banc durablement rouge n'apprend plus rien
        # — on cesse de le lire, et un VRAI échec passe inaperçu.
        #
        # D'où un registre `scripts/echecs-connus.txt` : chaque échec y est NOMMÉ,
        # DATÉ et ATTRIBUÉ, il est affiché à chaque exécution, mais il ne rend pas
        # le banc rouge. Un échec qu'on ne s'explique pas reste rouge — c'est
        # toute la valeur du registre.
        local registre="scripts/echecs-connus.txt"
        local inconnus=0 connus=0
        if [[ -f "$registre" ]]; then
            while IFS= read -r nom; do
                [[ -z "$nom" || "$nom" == \#* ]] && continue
                nom="${nom%%|*}"; nom="$(echo "$nom" | xargs)"
                if grep -aq "$nom" "$logfile"; then
                    connus=$((connus+1))
                    echo "    ⚠ échec CONNU (registre) : $nom"
                else
                    inconnus=$((inconnus+1))
                fi
            done < <(grep -avE '^\s*(#|$)' "$registre" | cut -d'|' -f1)
        fi
        if [[ $inconnus -eq 0 && $connus -gt 0 ]]; then
            PASS+=("$label (échec connu)")
            echo "  ✓ $connus échec(s) CONNU(S), aucun nouvel échec — voir $registre"
        else
            FAIL+=("$label")
            echo "  ✗ FAIL — see $logfile"
        fi
    fi
}

# ── AXE 5 : LE CORPUS RÉEL (requêtes du journal MCP) ────────
# Extrait en fonction : il tourne AUSSI en mode `open-only`, parce qu'il est
# court (une vingtaine de secondes) et que c'est exactement ce qu'on veut
# vérifier avant de committer : « aucune requête réelle n'a régressé ».
axe_corpus_reel() {
    echo "── Corpus réel : requêtes du journal MCP (seuil + ligne de base)"
    local store="${OBRAIN_BANC_STORE:-$HOME/bench-stores/runtime-dev-snapshot-20260909.obrain}"
    if [[ -d "$store" ]]; then
        # ── LE STORE PEUT ÊTRE TENU PAR UN AUTRE PROCESSUS (2026-09-15) ────
        #
        # Constaté en vrai : un serveur `viz serve` avait ouvert le banc figé, et
        # la porte refusait de mesurer (code 4) — le harnais marquait alors
        # l'axe en ÉCHEC, alors que rien n'était cassé. Un banc qui ne sait pas
        # mesurer dans son propre environnement ne fige rien.
        #
        # On replie donc sur la LECTURE SEULE (verrou partagé, qui coexiste),
        # en le DISANT — jamais en silence : mesurer un autre contexte sans le
        # signaler serait pire que ne pas mesurer.
        local ro=""
        local racine; racine="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
        local binaire="${OBRAIN_BIN:-$racine/target/release/obrain}"
        # ── ON CAPTURE, ON NE TUYAUTE PAS (2026-09-15) ─────────────────────
        #
        # Piège vécu : `… | grep -q` avec `set -o pipefail`. `grep -q` sort au
        # PREMIER succès et ferme le tuyau ; le producteur meurt alors de
        # SIGPIPE, et `pipefail` fait échouer la condition — la branche « store
        # tenu » n'était donc JAMAIS prise, on lisait toujours « sonde muette ».
        # L'issue restait la bonne (lecture seule), mais la RAISON était fausse.
        # On capture la réponse dans une variable, et on la montre.
        if "$binaire" query "$store" "RETURN 1" >/dev/null 2>&1; then
            : # inscriptible — on mesure le contexte normal
        elif { reponse="$("$binaire" query "$store" "RETURN 1" 2>&1 >/dev/null || true)"; } && \
             grep -q "already open by another process" <<<"$reponse"; then
            echo "    ⚠ store TENU par un autre processus → mesuré en LECTURE SEULE"
            ro="RO=1"
        else
            # ── ET SI LA SONDE NE RÉPOND PAS, ON LE DIT (2026-09-15) ───────
            #
            # Premier jet : la sonde était gardée par `[[ -x … ]]` et, le chemin
            # du binaire étant relatif, le garde sautait EN SILENCE — l'axe
            # échouait alors sans que personne ne sache pourquoi. Le voici
            # réparé : chemin absolu dérivé du script, et dans le doute la
            # LECTURE SEULE (qui coexiste toujours), avec la raison affichée.
            echo "    ⚠ la sonde d'ouverture n'a pas répondu — LECTURE SEULE par prudence"
            echo "      réponse : $(head -c 130 <<<"${reponse:-vide}" | tr '\n' ' ')"
            ro="RO=1"
        fi
        run_or_fail "requetes_lentes_corpus" \
            env $ro TOURS="${TOURS:-3}" SEUIL_MS="${OBRAIN_BANC_SEUIL:-50}" \
                TOLERANCE="${OBRAIN_BANC_TOLERANCE:-1.5}" \
                bash scripts/requetes-lentes.sh "$store"
    else
        echo "→ requetes_lentes_corpus — IGNORÉ (store absent : $store)"
        echo "    poser OBRAIN_BANC_STORE=/chemin/vers/store pour l'activer"
    fi
}

# ── AXE 6 : LE CHEMIN DU CHAT (bridge, runtime_search, context_build) ───────
# `scripts/bench-message-path.sh` RAPPORTE mais ne SANCTIONNE pas (il sort
# toujours 0). On ne modifie pas le script d'autrui : on l'enveloppe, et c'est
# l'enveloppe qui porte le seuil. Toute ligne « … chaud : med X ms » doit rester
# sous OBRAIN_CHAT_SEUIL_MS (défaut 50).
axe_chemin_du_chat() {
    echo "── Chemin du chat (bridge + runtime_search + context_build)"
    local seuil="${OBRAIN_CHAT_SEUIL_MS:-50}"
    local sortie
    sortie="$(VIZ_STORE="${OBRAIN_CHAT_STORE:-$HOME/bench-stores/runtime-dev-snapshot-20260909.obrain}" \
        bash scripts/bench-message-path.sh 2>&1)" || true
    printf '%s\n' "$sortie" | tail -6 | sed 's/^/    /'
    local pire
    pire="$(printf '%s\n' "$sortie" | awk -F'med ' '/ chaud : med /{split($2,a," "); if (a[1]+0 > m) {m=a[1]+0; n=$1}} END{printf "%.1f %s", m+0, n}')"
    if awk -v p="${pire%% *}" -v s="$seuil" 'BEGIN{exit !(p+0 > s+0)}'; then
        FAIL+=("chemin_du_chat (${pire%% *} ms > ${seuil} ms)")
        echo "  ✗ FAIL — le plus lent : $pire"
    else
        PASS+=("chemin_du_chat")
        echo "    ✓ le plus lent : $pire (seuil ${seuil} ms)"
    fi
}

# ── AXE 7 : LES ENDPOINTS `viz serve` ──────────────────────────────────────
# `scripts/smoke-backend.sh` porte déjà ses seuils (chaud < 50 ms) et sort 1 en
# cas d'échec : on l'appelle tel quel. IGNORÉ — en le disant — si le serveur ne
# répond pas, pour que le harnais reste utilisable sans lui.
axe_store_vivant() {
    # ── L'AXE QUI MANQUAIT (2026-09-15) ────────────────────────────────────
    #
    # L'objectif nomme explicitement « store figé OU store vivant » — et les
    # sept autres axes ne mesurent QUE le banc figé. Le store vivant n'était
    # mesuré qu'à la main, tour après tour, sans rien qui le fige.
    #
    # Deux contextes, parce qu'ils ne se ressemblent pas :
    #   · CLI jetable, en LECTURE SEULE — le harness tient le store en
    #     exclusif, une ouverture en écriture échouerait (la porte le dit
    #     désormais : code 4) ;
    #   · processus LONG, avec les valeurs — c'est le contexte du pont MCP,
    #     et c'est le seul qui paie la construction d'index.
    #
    # Cet axe RAPPORTE plus qu'il ne juge : il n'existe pas de ligne de base
    # pour un store qu'on ne possède pas (les données changent sous nos pieds).
    # Il échoue donc seulement si la MESURE est impossible — jamais si le
    # chiffre déplaît.
    local vivant="${OBRAIN_STORE_VIVANT:-$HOME/.local/share/obrain-harness/data/runtime-dev.obrain}"
    if [[ ! -d "$vivant" ]]; then
        echo "── store vivant — IGNORÉ (pas de store en $vivant)"
        echo "    (poser OBRAIN_STORE_VIVANT=/chemin/vers/store pour l'activer)"
        return
    fi
    echo "── store vivant : $vivant"
    # PAS de `run_or_fail` ici, et c'est délibéré : sur un store qu'on ne
    # possède pas, la porte rend TOUJOURS un verdict strict non nul (aucune
    # ligne de base ne peut être figée pour des données qui bougent). Ce qui
    # doit échouer, c'est la MESURE — pas le chiffre.
    local jlog="$OUTDIR/store_vivant_cli_jetable.log"
    if RO=1 TOURS=1 bash scripts/requetes-lentes.sh "$vivant" >"$jlog" 2>&1; then
        echo "    (verdict strict : aucun dépassement)"
    fi
    if grep -aq "ÉCHEC DE MESURE" "$jlog"; then
        echo "    ✗ la mesure a échoué — voir $jlog"
        FAIL+=("store_vivant_cli_jetable")
    else
        grep -aE "AU SEUIL OBJECTIF|VERDICT" "$jlog" | sed 's/^/    /'
        PASS+=("store_vivant_cli_jetable")
    fi

    local ex="${OBRAIN_BANC_EXEMPLE:-target/release/examples/bench_texte_gql}"
    local corpus="scripts/corpus-requetes.txt"
    if [[ ! -x "$ex" ]]; then
        echo "── store vivant, processus long — IGNORÉ ($ex absent)"
        echo "    (cargo build --release -p obrain-engine --example bench_texte_gql)"
        return
    fi
    echo "── store vivant, processus LONG (le contexte du pont MCP)"
    local bav="$(mktemp)"
    if OBRAIN_LECTURE_SEULE=1 OBRAIN_TRIGRAMMES_DES_LA=1 OBRAIN_BANC_VALEURS=1 \
        timeout 900 "$ex" "$vivant" 3 "$corpus" >"$bav" 2>/dev/null; then
        grep -aE "^  [A-Z][0-9]+ " "$bav" | cut -c1-96 | sed 's/^/    /' | head -18
        local pires
        pires="$(grep -aE "^  [A-Z][0-9]+ " "$bav" | awk '{print $4}' | sed 's/ms//' | sort -g | tail -1)"
        echo "    pire min sur 18 : ${pires} ms"
        PASS+=("store_vivant_processus_long")
    else
        echo "    ✗ la mesure a échoué"
        FAIL+=("store_vivant_processus_long")
    fi
    rm -f "$bav"
}

axe_viz_serve() {
    local base="${OBRAIN_VIZ_BASE:-http://192.168.1.103:8432}"
    if ! curl -s -o /dev/null --max-time 3 "$base/api/viz/status"; then
        echo "── viz serve — IGNORÉ (pas de réponse sur $base)"
        echo "    (poser OBRAIN_VIZ_BASE=… pour viser un autre serveur)"
        return
    fi
    echo "── viz serve : endpoints et fraîcheur ($base)"
    run_or_fail "viz_serve_endpoints" bash scripts/smoke-backend.sh "$base"
}

# ── 1. Lib tests ────────────────────────────────────────────
echo
echo "── Phase 1 : lib tests"
run_or_fail "obrain-substrate lib (523 tests)" \
    cargo test -p obrain-substrate --lib --release
run_or_fail "obrain-engine lib (663 tests)" \
    cargo test -p obrain-engine --lib --release

# ── 2. DB open bench ────────────────────────────────────────
echo
echo "── Phase 2 : DB open (3 corpora)"
run_or_fail "t17i_db_open_bench" \
    cargo test -p obrain-engine --release --features cypher \
    --test t17i_db_open_bench -- --nocapture

if [[ "$MODE" == "open-only" ]]; then
    echo
    axe_corpus_reel
    echo
    axe_chemin_du_chat
    echo
    axe_store_vivant
aaxe_viz_serve
    echo
    echo "── open-only mode — stopping"
    echo "PASS: ${#PASS[@]} / FAIL: ${#FAIL[@]}"
    exit $(( ${#FAIL[@]} > 0 ))
fi

# ── 3. RAM bench ────────────────────────────────────────────
echo
echo "── Phase 3 : RAM bench"
run_or_fail "t17i_ram_bench" \
    cargo test -p obrain-engine --release --features cypher \
    --test t17i_ram_bench -- --nocapture

# ── 4. Full PO bench ────────────────────────────────────────
echo
echo "── Phase 4 : 26-query PO bench vs Neo4j"
run_or_fail "po_vs_neo4j_bench" \
    cargo test -p obrain-engine --release --features cypher \
    --test po_vs_neo4j_bench -- bench_po_vs_neo4j --nocapture --exact

# ── 5. T9b planner suite ────────────────────────────────────
echo
echo "── Phase 5 : T9b planner (snapshot + variance + bench gate)"
run_or_fail "t9_planner_rewrite_snapshot" \
    cargo test -p obrain-engine --release --features cypher \
    --test t9_planner_rewrite_snapshot -- --nocapture
run_or_fail "t9_planner_rewrite_variance" \
    cargo test -p obrain-engine --release --features cypher \
    --test t9_planner_rewrite_variance -- --nocapture
run_or_fail "t9_planner_rewrite_correctness" \
    cargo test -p obrain-engine --release --features cypher \
    --test t9_planner_rewrite_correctness bench_gate_most_connected_under_30ms \
    -- --nocapture

# ── 6. Corpus réel : latences (non-régression) ──────────────
echo
axe_corpus_reel

# ── 7. Les autres contextes nommés ──────────────────────────
echo
axe_chemin_du_chat
echo
axe_store_vivant
axe_viz_serve

# ── Recap ──────────────────────────────────────────────────
echo
echo "═══ RECAP ═══"
echo "PASS : ${#PASS[@]} / ${#PASS[@]}+${#FAIL[@]}"
for p in "${PASS[@]}"; do echo "  ✓ $p"; done
if [[ ${#FAIL[@]} -gt 0 ]]; then
    echo "FAIL :"
    for f in "${FAIL[@]}"; do echo "  ✗ $f"; done
    echo
    echo "Logs dans $OUTDIR/"
    exit 1
fi
echo
echo "Logs dans $OUTDIR/ pour diff avec baseline."
