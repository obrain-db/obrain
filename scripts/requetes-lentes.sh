#!/usr/bin/env bash
# LENTEUR — le banc qui doit pouvoir dire « plus une seule requête lente ».
#
# ── POURQUOI CE BANC EXISTE ─────────────────────────────────────────────────
#
# « Tout avoir en ns » ne se vérifie pas sur une requête choisie : il se vérifie
# sur le CORPUS DES REQUÊTES RÉELLEMENT EXÉCUTÉES. Celui-ci n'est pas inventé
# pour les besoins du banc — il est **extrait des journaux de session** (les
# appels `obrain_query` du pont MCP des 12-13/09/2026, store `runtime-dev`).
# Chaque entrée porte sa provenance en commentaire.
#
# ── CE QUE LE BANC MESURE, ET CE QU'IL N'EST PAS ────────────────────────────
#
# Il mesure le TEMPS D'UNE REQUÊTE dans un processus qui vient de s'ouvrir —
# le pire cas, celui du CLI (`obrain query`) : ouverture + plan + exécution.
# Un pont MCP longue durée fait mieux (les index y sont bâtis une fois), donc
# ce banc est conservateur : ce qui passe ici passe partout.
#
# ── USAGE ───────────────────────────────────────────────────────────────────
#
#   scripts/requetes-lentes.sh [STORE] [SEUIL_MS]
#
#   STORE      défaut : le banc figé ~/bench-stores/runtime-dev-snapshot-20260909.obrain
#   SEUIL_MS   défaut : 50 — le seuil OBJECTIF de la campagne
#   OBRAIN_BIN surcharge le binaire · TOURS (défaut 3) · TIMEOUT (défaut 600)
#   RO=1       ouvre le store en LECTURE SEULE (store vivant, verrou partagé)
#   TOLERANCE  tolérance sur la ligne de base (défaut 1,5)
#   ECRIRE_BASELINE=1  écrit la ligne de base à partir de ce passage, puis sort
#
# ── DEUX VERDICTS, ET POURQUOI IL EN FAUT DEUX ──────────────────────────────
#
# Un banc qui clignote au seuil ne détecte RIEN : C9 mesurait 47,8 ms un passage
# et 67 ms le suivant, selon la charge de la machine. Confondre « objectif pas
# encore atteint » et « régression » rendrait la porte inutilisable en
# pré-commit.
#
# D'où la LIGNE DE BASE (`scripts/banc-requetes-lentes.baseline`) : le minimum
# mesuré de chaque requête, figé. Le CODE DE SORTIE ne sanctionne qu'une
# RÉGRESSION — une requête au-dessus de `max(SEUIL, base × TOLERANCE)` —, et le
# rapport affiche À PART le compte strict au seuil objectif. L'objectif reste
# donc visible, et la porte reste utile.
#
# Sans fichier de ligne de base : la porte retombe sur le comportement strict
# (sortie 1 dès qu'une requête dépasse le seuil).

set -uo pipefail

STORE="${1:-$HOME/bench-stores/runtime-dev-snapshot-20260909.obrain}"
SEUIL_MS="${2:-50}"
RACINE="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${OBRAIN_BIN:-$RACINE/target/release/obrain}"
TOURS="${TOURS:-3}"
TIMEOUT="${TIMEOUT:-600}"
# RO=1 : ouvre le store en LECTURE SEULE (verrou partagé) — indispensable pour
# mesurer un store VIVANT, tenu par le pont MCP du harness, sans l'arrêter.
RO="${RO:-}"
[[ "$RO" == "1" ]] && RO="--read-only"
BASELINE="${BASELINE:-$RACINE/scripts/banc-requetes-lentes.baseline}"
TOLERANCE="${TOLERANCE:-1.5}"

# La ligne de base ne vaut que pour SON store (elle est nommée : `#store`).
BASE_STORE="$(awk -F'\t' '$1=="#store"{print $2}' "$BASELINE" 2>/dev/null | head -1)"
BASE_IGNOREE=""
if [[ -f "$BASELINE" && "$BASE_STORE" != "$STORE" ]]; then
  BASE_IGNOREE=1
fi

[[ -x "$BIN" ]] || { echo "binaire absent : $BIN" >&2; exit 2; }

# ── LE STORE EST-IL TENU PAR UN AUTRE PROCESSUS ? (2026-09-15) ──────────────
#
# MÊME FAMILLE QUE « INSTRUMENT PÉRIMÉ » : une mesure IMPOSSIBLE doit le dire.
# Constaté : le serveur `viz serve` tenait le banc figé en exclusif (ouvert par
# une requête de mesure), et les 18 requêtes rendaient « ÉCHEC DE MESURE » — un
# verdict qui ressemble à une panne du moteur alors que c'est un verrou.
#
# Une ouverture en LECTURE SEULE prend un verrou PARTAGÉ et coexiste : c'est la
# sortie de secours, et elle est indiquée.
if [[ -z "$RO" ]] && [[ -d "$STORE" ]]; then
  tenu="$(timeout 60 "$BIN" query "$STORE" "RETURN 1" 2>&1 >/dev/null | grep -ac "already open by another process")"
  if [[ "$tenu" != "0" ]]; then
    echo "STORE TENU : $STORE est ouvert par un autre processus." >&2
    echo "  → RO=1 $0 $STORE   (lecture seule : verrou partagé, coexiste)" >&2
    exit 4
  fi
fi

# ── LE BINAIRE MESURÉ EST-IL À JOUR ? (2026-09-14) ──────────────────────────
#
# POURQUOI CETTE GARDE EXISTE. Deux fois pendant ce chantier j'ai mesuré un
# binaire PÉRIMÉ et conclu sur du vide : j'avais changé le moteur, lancé
# `cargo check` (qui ne produit rien) ou reconstruit un AUTRE binaire, puis
# mesuré l'ancien. La première fois j'en ai déduit que « le CLI n'atteint pas le
# crochet » — une demi-heure perdue ; la seconde, que ma parallélisation
# n'apportait rien.
#
# Une porte qui mesure un binaire périmé est PIRE qu'aucune porte : elle donne
# un verdict vert sur du code qui n'est plus là. On refuse donc de mesurer, avec
# un code de sortie DISTINCT (3) pour qu'on ne confonde pas « instrument périmé »
# avec « requête lente ».
#
# `ACCEPTER_BINAIRE_PERIME=1` force la mesure (utile pour comparer deux époques).
if [[ "$BIN" == "$RACINE/target/release/obrain" ]]; then
  perime="$(find "$RACINE/crates" -name '*.rs' -newer "$BIN" -print -quit 2>/dev/null)"
  if [[ -n "$perime" && "${ACCEPTER_BINAIRE_PERIME:-}" != "1" ]]; then
    echo "INSTRUMENT PÉRIMÉ : $BIN est plus ancien que" >&2
    echo "                    ${perime#"$RACINE/"}" >&2
    echo "  → cargo build --release -p obrain-cli" >&2
    echo "  (ou ACCEPTER_BINAIRE_PERIME=1 pour mesurer quand même)" >&2
    exit 3
  fi
fi
[[ -d "$STORE" ]] || { echo "store absent : $STORE" >&2; exit 2; }

# ── LE CORPUS ───────────────────────────────────────────────────────────────
# Chaque entrée : « libellé|requête ». Provenance : journal MCP du 2026-09-12
# pour les six premières (elles ont ouvert le chantier perf), puis celles que
# les sessions ont réellement posées ensuite.
CORPUS=(
  "C1 name CONTAINS iam (3 termes)|MATCH (n) WHERE n.name CONTAINS 'iam' OR n.name CONTAINS 'IAM' OR n.qualified_name CONTAINS 'iam' RETURN n.qualified_name, labels(n) LIMIT 30"
  "C2 auth/token (4 termes)|MATCH (n) WHERE n.name CONTAINS 'auth' OR n.name CONTAINS 'Auth' OR n.name CONTAINS 'token' OR n.name CONTAINS 'Token' RETURN n.qualified_name, labels(n) LIMIT 30"
  "C3 STARTS WITH module|MATCH (n) WHERE n.qualified_name STARTS WITH 'home.triviere.projects.banc_obrain.crates.obrain-iam' RETURN n.qualified_name, labels(n) LIMIT 60"
  "C4 STARTS WITH + labels (OU)|MATCH (n) WHERE n.qualified_name STARTS WITH 'home.triviere.projects.banc_obrain.crates.obrain-iam.src' AND (n:Module OR n:Struct OR n:Enum OR n:Trait OR n:Class OR n:Interface OR n:Type) RETURN n.qualified_name, labels(n) LIMIT 40"
  "C5 File CONTAINS obrain-iam|MATCH (f:File) WHERE f.qualified_name CONTAINS 'obrain-iam' RETURN f.qualified_name LIMIT 40"
  "C6 aretes traversantes|MATCH (a)-[r]->(b) WHERE a.qualified_name CONTAINS 'obrain-iam' AND NOT b.qualified_name CONTAINS 'obrain-iam' RETURN a.qualified_name, type(r), b.qualified_name LIMIT 40"
  "C7 aretes TESTS|MATCH (a)-[r:TESTS]->(b) WHERE a.qualified_name CONTAINS 'obrain-iam' RETURN a.qualified_name, b.qualified_name LIMIT 30"
  "C8 STARTS WITH + Function|MATCH (n) WHERE n.qualified_name STARTS WITH 'home.triviere.projects.banc_obrain.crates.obrain-iam.src' AND (n:Function OR n:Method) RETURN n.qualified_name LIMIT 80"
  "C9 intervalle de qn|MATCH (n) WHERE n.qualified_name >= 'home.triviere.projects.banc_obrain.crates.obrain-iam' AND n.qualified_name < 'home.triviere.projects.banc_obrain.crates.obrain-iamz' RETURN n.qualified_name LIMIT 60"
  "C10 egalite indexee|MATCH (n) WHERE n.qualified_name = 'home.triviere.projects.banc_obrain.workspace.dependencies.obrain-iam' RETURN n.qualified_name LIMIT 5"
  "C11 balayage de label|MATCH (m:Method) RETURN m.qualified_name, m.file_path LIMIT 25"
  "C12 messages d'une session|MATCH (m:Message) WHERE m.session = 'session-x' AND m.role = 'user' RETURN m.seq AS seq, m.text AS text ORDER BY m.seq DESC LIMIT 3"
  "C13 compteur de noeuds|MATCH (n) RETURN count(n)"
  # ── FORMES RÉELLES AJOUTÉES LE 2026-09-13 ──────────────────────────────
  # Elles ne viennent pas des journaux mais des requêtes POSÉES PENDANT LA
  # CAMPAGNE — donc réelles elles aussi. Mesurées, et TROIS dépassent le seuil :
  # c'est un fait à suivre, pas à cacher.
  #   F1 : lookup à UNE borne du store ⇒ dégénère en balayage (chantier index
  #        ordonné). F3 : plage bornée à préfixe NON sélectif ⇒ le chemin
  #        d'intervalle coûte 3,4× un balayage (défaut de choix de plan).
  #   F7 : une négation ne s'indexe pas — plancher structurel du balayage.
  "F1 plage basse seule|MATCH (n) WHERE n.qualified_name >= 'home.triviere.projects.banc_obrain.crates.obrain-iam.src.libzzzz' RETURN count(n)"
  "F3 plage prefixe non selectif|MATCH (n) WHERE n.name >= 'aa' AND n.name < 'ab' RETURN count(n)"
  "F7 negation seule|MATCH (n) WHERE NOT n.qualified_name CONTAINS 'obrain' RETURN count(n)"
  # ── CONTEXTE `viz serve` (2026-09-13) ──────────────────────────────────
  # La requête du handler `/api/viz/space` — `CONTAINS '.prefix.'` sur le qn.
  # C'est LA requête de ce contexte, et elle est lente pour une raison qu'aucun
  # index ne peut corriger : « .crates. » est fait de trigrammes TRÈS COMMUNS
  # (`.cr`, `cra`, `ate`, `tes`, `es.`), donc l'index se TAIT — à juste titre —
  # et le balayage reste. Mesuré identique avec et sans index (67–74 ms).
  "V1 viz space (CONTAINS .crates.)|MATCH (n) WHERE n.qualified_name CONTAINS '.crates.' RETURN n.qualified_name LIMIT 43"
  "C14 compteur de label|MATCH (f:File) RETURN count(f)"
)

# Le CLI rend « 1 row (205us) » aussi bien que « (1.23ms) » : les deux unités.
en_ms() {
  case "$1" in
    *us) awk -v v="${1%us}" 'BEGIN{printf "%.3f", v/1000}' ;;
    *ms) printf '%s' "${1%ms}" ;;
    *s)  awk -v v="${1%s}"  'BEGIN{printf "%.1f", v*1000}' ;;
    *)   printf '0' ;;
  esac
}

mesure() {
  local q="$1" t=() m=() v s e
  for _ in $(seq "$TOURS"); do
    s=$(date +%s%N)
    v=$(timeout "$TIMEOUT" "$BIN" query "$STORE" "$q" $RO --timing --format table 2>/dev/null \
        | grep -oE '\([0-9.]+(us|ms|s)\)' | tail -1 | tr -d '()')
    e=$(date +%s%N)
    [[ -n "$v" ]] && t+=("$(en_ms "$v")")
    m+=("$(( (e-s)/1000000 ))")
  done
  [[ ${#t[@]} -eq 0 ]] && { printf 'ÉCHEC'; return; }
  # Temps de REQUÊTE (le chronomètre du moteur) ET temps MURAL du processus.
  #
  # ── POURQUOI LES DEUX (2026-09-15) ──────────────────────────────────────
  #
  # Le seuil objectif porte sur la REQUÊTE, mais un utilisateur subit le
  # PROCESSUS : ouverture + requête. Sur le store vivant, l'ouverture a coûté
  # jusqu'à 1 400 ms — et le verdict « requête » ne le voyait pas. Constaté
  # aussi dans l'autre sens : une requête d'égalité passée de 0,35 à 331 ms
  # parce que la construction d'index a été DIFFÉRÉE, alors que le processus,
  # lui, est passé de ~1 350 à ~480 ms. Un seul chiffre ne peut pas dire les
  # deux ; on affiche les deux, et le VERDICT reste sur la requête (le seuil de
  # la campagne).
  local mur
  mur=$(printf '%s\n' "${m[@]}" | sort -n | head -1)
  local req
  req=$(printf '%s\n' "${t[@]}" | sort -g | awk '{v[NR]=$1} END{printf "%.2f %.2f", v[1], v[int((NR+1)/2)]}')
  printf '%s %s' "$req" "$mur"
}

echo "store  : $STORE"
echo "binaire: $BIN"
echo "seuil  : ${SEUIL_MS} ms   (tours : $TOURS, minimum et médiane)"
if [[ -f "$BASELINE" && -z "$BASE_IGNOREE" ]]; then
  echo "base   : $(basename "$BASELINE") — régression au-delà de max(seuil, base × $TOLERANCE)"
else
  BASE_IGNOREE=1
  if [[ -f "$BASELINE" ]]; then
    echo "base   : IGNORÉE — elle décrit un autre store (${BASE_STORE:-inconnu}) : verdict STRICT"
  else
    echo "base   : (aucune — verdict STRICT au seuil)"
  fi
fi
echo
printf '%-30s %10s %10s %9s   %s\n' "requête" "min (ms)" "méd (ms)" "mural" "verdict"
printf '%s\n' "------------------------------------------------------------------------------------------"

if [[ "${ECRIRE_BASELINE:-}" == "1" ]]; then
  echo "écriture de la ligne de base : $BASELINE"
  # La ligne de base NOMME son store : les temps n'ont de sens que pour lui.
  # Sans cela, mesurer un AUTRE store rendait de faux verdicts de régression
  # (constaté : le store vivant, deux fois plus gros, comparé au banc figé).
  printf '#store\t%s\n' "$STORE" > "$BASELINE"
fi

LENTES=0; TOTAL=0; STRICT=0
RESULTATS=""
for entree in "${CORPUS[@]}"; do
  nom="${entree%%|*}"; q="${entree#*|}"
  lecture=$(mesure "$q")
  if [[ "$lecture" == "ÉCHEC" ]]; then
    printf '%-30s %10s %10s   %s\n' "$nom" "—" "—" "ÉCHEC DE MESURE"
    LENTES=$((LENTES+1)); TOTAL=$((TOTAL+1)); continue
  fi
  read -r mini medi mur <<<"$lecture"

  # LE SEUIL EFFECTIF : la ligne de base, si elle existe pour cette requête.
  base=""
  [[ -z "$BASE_IGNOREE" ]] && base="$(awk -F'\t' -v n="$nom" '$1==n{print $2}' "$BASELINE" 2>/dev/null | head -1)"
  seuil_eff="$SEUIL_MS"
  if [[ -n "$base" ]]; then
    seuil_eff=$(awk -v b="$base" -v t="$TOLERANCE" -v s="$SEUIL_MS" \
      'BEGIN{e=b*t; print (e>s+0) ? e : s+0}')
  fi

  verdict=$(awk -v m="$mini" -v s="$seuil_eff" 'BEGIN{print (m+0 > s+0) ? "REGRESSION ✗" : "ok"}')
  [[ "$verdict" == REGRESSION* ]] && LENTES=$((LENTES+1))
  [[ "$mini" =~ ^[0-9.]+$ ]] && awk -v m="$mini" -v s="$SEUIL_MS" 'BEGIN{exit !(m+0 > s+0)}' && STRICT=$((STRICT+1))
  TOTAL=$((TOTAL+1))
  note=""
  [[ -n "$base" ]] && note=$(printf '   (base %s ms × %s)' "$base" "$TOLERANCE")
  [[ "${ECRIRE_BASELINE:-}" == "1" && "$mini" =~ ^[0-9.]+$ ]] && printf '%s\t%s\n' "$nom" "$mini" >> "$BASELINE"
  RESULTATS+="$(printf '%s\t%s' "$medi" "$(printf '%-30s %10s %10s %9s   %s%s' "$nom" "$mini" "$medi" "$mur" "$verdict" "$note")")"$'\n'
done

printf '%s' "$RESULTATS" | sort -g | cut -f2-

echo
if [[ "${ECRIRE_BASELINE:-}" == "1" ]]; then
  echo "LIGNE DE BASE ÉCRITE : $TOTAL requête(s) dans $BASELINE"
  exit 0
fi
echo "AU SEUIL OBJECTIF : ${STRICT} requête(s) sur ${TOTAL} au-dessus de ${SEUIL_MS} ms."
if [[ $LENTES -eq 0 ]]; then
  echo "VERDICT : aucune RÉGRESSION sur ${TOTAL} requêtes du corpus réel."
  exit 0
fi
if [[ -n "$BASE_IGNOREE" ]]; then
  echo "VERDICT : ${LENTES} requête(s) sur ${TOTAL} au-dessus du seuil STRICT de ${SEUIL_MS} ms. ✗"
else
  echo "VERDICT : ${LENTES} RÉGRESSION(S) sur ${TOTAL} requêtes du corpus réel. ✗"
fi
exit 1
