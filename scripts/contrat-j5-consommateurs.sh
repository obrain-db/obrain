#!/usr/bin/env bash
# CONTRAT J5 — batterie consommateurs v3 (jalons B et C du plan-j5.md).
#
# Chaque sous-commande est une CLAUSE du contrat : elle sort en 0 si la
# clause tient, en 1 sinon, et imprime des lignes `J5|<clause>|OK/ECHEC|...`
# faites pour etre archivees telles quelles dans le journal du chantier.
#
# Aucune sous-commande ne touche un store vivant : tout chemin de store passe
# en argument et doit pointer sous ~/tmp/harness-j5/.
set -u

die() { echo "J5|$1|ECHEC|$2"; exit 1; }
ok()  { echo "J5|$1|OK|$2"; }

# Normalisation JSON stable (tri des cles, suppression des champs de chrono).
# NB : PAS de heredoc ici — un heredoc remplacerait le stdin du pipe.
NORM_PY='
import json,sys
VOLATILS={"duration_ms","elapsed_ms","timing","took_ms","duree_ms","cache","execution_time","execution_time_ms"}
def strip(o):
    if isinstance(o,dict):
        return {k:strip(v) for k,v in sorted(o.items()) if k not in VOLATILS}
    if isinstance(o,list):
        return [strip(x) for x in o]
    return o
raw=sys.stdin.read()
try:
    print(json.dumps(strip(json.loads(raw)),sort_keys=True,ensure_ascii=False,indent=1))
except Exception:
    sys.stdout.write(raw)  # pas du JSON : brut (le diff tranchera)
'
normaliser() { python3 -c "$NORM_PY"; }

# Etat disque des zones PORTEUSES DE PROPS d'un store (contrat C6 / B6).
empreinte_props() { # $1=store
  (cd "$1" && ls -l substrate.props* substrate.blobcol.* substrate.strings substrate.dict 2>/dev/null \
     | awk '{print $9, $5}' | sort)
}

# Empreinte complete (refus croise : RIEN ne doit bouger).
#
# Le side-car d'ancre de LSN (`substrate.wal.lsn`) et son temporaire
# (`substrate.wal.lsn.tmp`) sont EXCLUS : ils decrivent le WAL, ils ne portent
# aucune donnee du graphe, et leur contenu/taille bougent legitimement a chaque
# ouverture d'une base neuve, a chaque checkpoint et a chaque reparation de
# queue. Les inclure rendait la clause `refus-croise` NON DETERMINISTE — le
# temporaire n'existe que pendant le `write`+`rename` d'une publication.
empreinte_totale() { # $1=store
  (cd "$1" && for f in substrate.*; do
     case "$f" in substrate.wal.lsn|substrate.wal.lsn.tmp) continue;; esac
     printf '%s %s %s\n' "$f" "$(stat -c%s "$f")" "$(sha256sum "$f" | cut -d" " -f1)"
   done | sort)
}

# Inode+taille+mtime de la zone nodes (idempotence : pas de reecriture).
etat_nodes() { stat -c '%i %s %Y' "$1/substrate.nodes"; }

case "${1:-}" in

# ---------------------------------------------------------------- comptages
# comptages <binaire> <store> <sortie.txt> [label...]
# Baseline/relecture : total noeuds, total aretes, count par label,
# echantillon de props (20 noeuds tries par id).
comptages)
  BIN=$2; STORE=$3; OUT=$4; shift 4
  : > "$OUT"
  {
    echo "== total"
    "$BIN" query "$STORE" 'MATCH (n) RETURN count(n)' | normaliser
    echo "== aretes"
    "$BIN" query "$STORE" 'MATCH (a)-[r]->(b) RETURN count(r)' | normaliser
    for L in "$@"; do
      echo "== label $L"
      "$BIN" query "$STORE" "MATCH (n:$L) RETURN count(n)" | normaliser
    done
    echo "== echantillon props"
    "$BIN" query "$STORE" 'MATCH (n) RETURN n LIMIT 20' | normaliser
  } >> "$OUT" 2>"$OUT.stderr"
  grep -q '"count' "$OUT" || grep -q '{' "$OUT" || die comptages "aucun resultat dans $OUT"
  ok comptages "$OUT"
  ;;

# ---------------------------------------------------------------- panel
# panel <base_url> <store_param> <panel.tsv> <nonce> <outdir>
# Execute le panel (nom<TAB>requete) via GET /api/query du viz.
# Le NONCE est insere en espacement dans la requete (anti-cache).
panel)
  URL=$2; SP=$3; TSV=$4; NONCE=$5; OUTDIR=$6
  mkdir -p "$OUTDIR"
  RC=0
  while IFS=$'\t' read -r nom req; do
    [ -z "$nom" ] && continue
    # anti-cache : espaces surnumeraires fonction du nonce
    pad=$(printf '%*s' $(( (NONCE % 7) + 1 )) '')
    q=$(python3 -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.argv[1]))' "MATCH$pad${req#MATCH}")
    # SP = "-" : pas de parametre store (le store par defaut du serveur)
    sp=""; [ "$SP" != "-" ] && sp="store=$(python3 -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.argv[1]))' "$SP")&"
    curl -sS --max-time 300 "$URL/api/query?$sp""query=$q" | normaliser > "$OUTDIR/$nom.json" \
      || { echo "J5|panel|ECHEC|$nom"; RC=1; }
    [ -s "$OUTDIR/$nom.json" ] || { echo "J5|panel|ECHEC|$nom vide"; RC=1; }
  done < "$TSV"
  [ $RC -eq 0 ] && ok panel "$OUTDIR" || exit 1
  ;;

# ---------------------------------------------------------------- compare
# compare <dirA> <dirB> : les reponses normalisees doivent etre IDENTIQUES.
compare)
  A=$2; B=$3
  if diff -ru "$A" "$B" > /tmp/j5-diff.$$ 2>&1; then
    ok compare "$A == $B"
  else
    sed -n '1,40p' /tmp/j5-diff.$$
    die compare "$A != $B (diff ci-dessus)"
  fi
  ;;

# ---------------------------------------------------------------- migrer
# migrer <binaire_v3> <store> : ouverture exclusive = migration. Chrono
# consigne (INFORMATION seulement — machine chargee).
migrer)
  BIN=$2; STORE=$3
  T0=$(date +%s)
  "$BIN" query "$STORE" 'MATCH (n) RETURN count(n)' > /tmp/j5-migre.$$ 2>&1 \
    || { sed -n '1,10p' /tmp/j5-migre.$$; die migrer "ouverture v3 en echec"; }
  T1=$(date +%s)
  ok migrer "$STORE en $((T1-T0))s (chrono informatif) : $(grep -o '"count[^,}]*' /tmp/j5-migre.$$ | head -1)"
  ;;

# ---------------------------------------------------------------- idempotence
# idempotence <binaire_v3> <store> : une SECONDE ouverture ne re-migre pas —
# la zone nodes (reecrite+rename par la migration 2->3) garde inode, taille
# et mtime.
idempotence)
  BIN=$2; STORE=$3
  AV=$(etat_nodes "$STORE")
  "$BIN" query "$STORE" 'RETURN 1' > /dev/null 2>&1 || die idempotence "reouverture en echec"
  AP=$(etat_nodes "$STORE")
  [ "$AV" = "$AP" ] || die idempotence "zone nodes reecrite ($AV -> $AP)"
  ok idempotence "zone nodes intacte ($AP)"
  ;;

# ---------------------------------------------------------------- refus-croise
# refus-croise <binaire_v1> <binaire_v3> <store_v3> : le v1 refuse PROPREMENT
# (exit != 0) et ne corrompt rien (empreinte sha256 totale identique), le v3
# relit toujours.
refus-croise)
  V1=$2; V3=$3; STORE=$4
  AV=$(empreinte_totale "$STORE")
  if "$V1" query "$STORE" 'RETURN 1' > /tmp/j5-v1.$$ 2>&1; then
    die refus-croise "le binaire v1 a ACCEPTE un store v3"
  fi
  grep -qi 'version\|format' /tmp/j5-v1.$$ || echo "J5|refus-croise|NOTE|message v1 sans mention de version : $(head -c200 /tmp/j5-v1.$$)"
  AP=$(empreinte_totale "$STORE")
  [ "$AV" = "$AP" ] || die refus-croise "le refus v1 a MODIFIE le store"
  "$V3" query "$STORE" 'MATCH (n) RETURN count(n)' > /dev/null 2>&1 || die refus-croise "le v3 ne relit plus apres la tentative v1"
  ok refus-croise "refus propre, empreinte intacte, v3 relit"
  ;;

# ---------------------------------------------------------------- props-stables
# props-stables <binaire_v3> <store> : 3 cycles open/close, zones props/blobcol
# strictement figees (contre-epreuve BUG-PROPS-TRUNCATE sur store reel).
props-stables)
  BIN=$2; STORE=$3
  AV=$(empreinte_props "$STORE")
  for c in 1 2 3; do
    "$BIN" query "$STORE" 'MATCH (n) RETURN n LIMIT 5' > /tmp/j5-props.$c.$$ 2>/dev/null \
      || die props-stables "cycle $c : ouverture en echec"
    AP=$(empreinte_props "$STORE")
    [ "$AV" = "$AP" ] || { diff <(echo "$AV") <(echo "$AP"); die props-stables "cycle $c : une zone props/blobcol a bouge"; }
  done
  cmp -s /tmp/j5-props.1.$$ /tmp/j5-props.3.$$ || die props-stables "les props relues different entre cycles"
  ok props-stables "3 cycles, zones figees, props identiques"
  ;;

# ---------------------------------------------------------------- slots
# slots <binaire_v3> <store> <requete> <attendu_max> : execute avec --slots,
# extrait le compteur, exige slots <= attendu_max. C'est LA preuve de route
# indexee (0 = aucun balayage de zone nodes).
slots)
  BIN=$2; STORE=$3; REQ=$4; MAX=$5
  OUT=$("$BIN" query --slots "$STORE" "$REQ" 2>&1 >/dev/null) || die slots "requete en echec : $OUT"
  N=$(printf '%s\n' "$OUT" | grep -o 'slots_noeuds_lus[ =:]*[0-9]*' | grep -o '[0-9]*$' | head -1)
  [ -n "$N" ] || die slots "compteur absent de stderr : $OUT"
  [ "$N" -le "$MAX" ] || die slots "slots=$N > $MAX pour : $REQ"
  ok slots "slots=$N (<= $MAX) pour : $REQ"
  ;;

# ---------------------------------------------------------------- parite
# parite <bin_v1> <store_v1> <bin_v3> <store_v3> <requete> <ratio_max_x10>
# (NATIF-J5B) Borne RELATIVE v3/v1 du temps REQUETE-SEULE au CLI :
# medianes de 5 passes ENTRELACEES (v1,v3,v1,v3,...) du temps total, moins
# la mediane de 5 ouvertures pures (RETURN 1). Jamais de chrono absolu :
# la machine est chargee, seule la comparaison simultanee des deux binaires
# sur les memes octets fait foi.
parite)
  B1=$2; S1=$3; B3=$4; S3=$5; REQ=$6; RMAX=$7
  chrono() { /usr/bin/time -f "%e" "$1" query "$2" "$3" >/dev/null 2>/tmp/j5-c.$$; tail -1 /tmp/j5-c.$$; }
  mediane() { sort -n | awk "{a[NR]=\$1} END {print a[int((NR+1)/2)]}"; }
  O1=$(for i in 1 2 3 4 5; do chrono "$B1" "$S1" "RETURN 1"; done | mediane)
  O3=$(for i in 1 2 3 4 5; do chrono "$B3" "$S3" "RETURN 1"; done | mediane)
  T1=$(for i in 1 2 3 4 5; do chrono "$B1" "$S1" "$REQ"; chrono "$B3" "$S3" "$REQ" >> /tmp/j5-t3.$$; done | mediane)
  T3=$(mediane < /tmp/j5-t3.$$); rm -f /tmp/j5-t3.$$
  R=$(python3 -c "
o1,o3,t1,t3=$O1,$O3,$T1,$T3
q1=max(t1-o1,0.001); q3=max(t3-o3,0.001)
print(f'{q3/q1:.2f} q1={q1:.3f}s q3={q3:.3f}s')")
  RA=$(echo "$R" | cut -d" " -f1)
  python3 -c "exit(0 if float(\"$RA\")*10 <= $RMAX else 1)" \
    && ok parite "ratio v3/v1 = $R (<= $(python3 -c "print($RMAX/10)")) : $REQ" \
    || die parite "ratio v3/v1 = $R > $(python3 -c "print($RMAX/10)") : $REQ"
  ;;

*)
  echo "usage: $0 {comptages|panel|compare|migrer|idempotence|refus-croise|props-stables|slots|parite} ..." >&2
  exit 2
  ;;
esac
