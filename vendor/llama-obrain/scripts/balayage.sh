#!/usr/bin/env bash
# balayage.sh — un POINT de politique de quantification : quantifier, mesurer, consigner.
#
# Pourquoi un script plutôt qu'une commande tapée à la main : chaque point coûte ~4 min de
# quantification + ~5 min de perplexité, et le balayage précédent a perdu ses artefacts parce qu'il
# écrivait dans /tmp (qui est sur la racine, à 98 %). Ici : sortie sur /mnt, journal TSV append-only,
# et un point déjà mesuré n'est pas repayé (le journal fait foi).
#
# Usage :
#   scripts/balayage.sh ref          # contre-épreuve : reproduction du PPL connu de OBRAIN-v2 (4,74)
#   scripts/balayage.sh combo        # attn_qkv + ssm_out  (le point prédit ~4,0, jamais mesuré)
#   scripts/balayage.sh qkv5         # attn_qkv en Q5_K    (le « bit manquant » sur l'attention)
#   scripts/balayage.sh qkv6         # attn_qkv en Q6_K
#   scripts/balayage.sh v5           # attn_v   en Q5_K
#   scripts/balayage.sh liste        # n'affiche que le journal
#
# Le corpus et les drapeaux de perplexité sont FIGÉS ici pour que tous les points soient
# comparables entre eux ET avec la table de descente de docs/REPRISE-TERNARISATION-2026-09-20.md §2.

set -uo pipefail

RACINE=/home/triviere/projects/llama-obrain
BUILD=$RACINE/build
MODELES=/mnt/obrain/models
SORTIE=$MODELES/balayage
CORPUS=/mnt/obrain/corpus/ppl.txt
JOURNAL=$SORTIE/journal.tsv
SOURCE=$MODELES/Qwen3.8-27B-BF16.gguf
V2=$MODELES/Qwen3.8-27B-OBRAIN-v2.gguf

# la base commune à tous les points : c'est la politique du candidat sauvegardé OBRAIN-v2
BASE=(OBRAIN_EDGE_LAYERS=8 OBRAIN_ATTN_V=Q4_K OBRAIN_FFN_DOWN=Q4_K)
BASE_NOM="bords8+attn_v+ffn_down"

mkdir -p "$SORTIE"

if [ "${1:-}" = "liste" ]; then
  if [ -s "$JOURNAL" ]; then column -t -s$'\t' "$JOURNAL"; else echo "(journal vide)"; fi
  exit 0
fi

POINT="${1:?usage: balayage.sh <ref|combo|qkv5|qkv6|v5|liste>}"

# ── la table des points : nom | variables supplémentaires | ce qu'on cherche à savoir ──
case "$POINT" in
  ref)   EXTRA=();                                        QUESTION="reproduction du PPL connu de OBRAIN-v2 (attendu 4,74) — valide l'instrument" ;;
  combo) EXTRA=(OBRAIN_ATTN_QKV=Q4_K OBRAIN_SSM_OUT=Q4_K); QUESTION="les deux meilleurs ajouts réunis (attendu ~4,0, JAMAIS MESURÉ)" ;;
  # Les trois points ci-dessous partent du MEILLEUR point connu (combo, 4,0825) et n'en changent
  # QU'UNE variable : c'est la seule façon d'attribuer un écart à une cause plutôt qu'à un paquet.
  qkv5)  EXTRA=(OBRAIN_ATTN_QKV=Q5_K OBRAIN_SSM_OUT=Q4_K);                    QUESTION="une variable de plus que combo : attn_qkv Q4_K -> Q5_K. Le bit manquant est-il sur l'attention ?" ;;
  qkv6)  EXTRA=(OBRAIN_ATTN_QKV=Q6_K OBRAIN_SSM_OUT=Q4_K);                    QUESTION="idem en Q6_K — paye-t-on encore, ou est-ce saturé dès Q5_K ?" ;;
  v5)    EXTRA=(OBRAIN_ATTN_QKV=Q4_K OBRAIN_SSM_OUT=Q4_K OBRAIN_ATTN_V=Q5_K); QUESTION="une variable de plus que combo : attn_v Q4_K -> Q5_K" ;;
  *) echo "point inconnu : $POINT"; exit 2 ;;
esac

# le point `ref` ne requantifie pas : il re-mesure le fichier sauvegardé
if [ "$POINT" = "ref" ]; then
  GGUF="$V2"
else
  GGUF="$SORTIE/$POINT.gguf"
fi

echo "=== point $POINT — $(date -Is) ==="
echo "    politique : $BASE_NOM ${EXTRA[*]:-(aucun ajout)}"
echo "    question  : $QUESTION"

# ── 1) quantification (sautée si le fichier existe déjà) ──
if [ "$POINT" != "ref" ] && [ ! -s "$GGUF" ]; then
  echo "    [1/3] quantification -> $GGUF"
  env "${BASE[@]}" "${EXTRA[@]}" "$BUILD/bin/llama-quantize" "$SOURCE" "$GGUF" TQ2_G128 \
      > "$SORTIE/$POINT.quant.log" 2>&1
  rc=$?
  if [ $rc -ne 0 ]; then echo "    ÉCHEC quantification (code $rc) — voir $SORTIE/$POINT.quant.log"; tail -5 "$SORTIE/$POINT.quant.log"; exit 1; fi
else
  echo "    [1/3] quantification : déjà présente, on ne repaye pas"
fi

TAILLE=$(stat -c %s "$GGUF")

# ── 2) perplexité — drapeaux FIGÉS, identiques à la table de descente ──
echo "    [2/3] perplexité (corpus $(sha256sum "$CORPUS" | cut -c1-8)…)"
T0=$(date +%s)
"$BUILD/bin/llama-perplexity" -m "$GGUF" -f "$CORPUS" -c 512 --chunks 2 -t 8 -ngl 0 \
    > "$SORTIE/$POINT.ppl.log" 2>&1
rc=$?
T1=$(date +%s)
if [ $rc -ne 0 ]; then echo "    ÉCHEC perplexité (code $rc) — voir $SORTIE/$POINT.ppl.log"; tail -5 "$SORTIE/$POINT.ppl.log"; exit 1; fi

PPL=$(grep -oE "Final estimate: PPL = [0-9.]+" "$SORTIE/$POINT.ppl.log" | grep -oE "[0-9.]+$" | tail -1)
ERR=$(grep -oE "Final estimate: PPL = [0-9.]+ +/- [0-9.]+" "$SORTIE/$POINT.ppl.log" | grep -oE "\+/- [0-9.]+" | tail -1)
[ -z "$PPL" ] && PPL="?"
echo "    PPL = $PPL $ERR   ($(( T1 - T0 )) s, $(( TAILLE / 1000000 )) Mo)"

# ── 3) consignation ──
printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n" \
  "$(date -Is)" "$POINT" "$BASE_NOM ${EXTRA[*]:-—}" "$TAILLE" "$PPL" "${ERR:-—}" "$(( T1 - T0 ))" "$QUESTION" \
  >> "$JOURNAL"
echo "    [3/3] consigné dans $JOURNAL"
echo "=== point $POINT TERMINÉ — $(date -Is) ==="
