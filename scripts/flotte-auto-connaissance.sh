#!/usr/bin/env bash
# ── SONDE D'AUTO-CONNAISSANCE DE LA FLOTTE ───────────────────────────────────
#
# À quoi ça sert, et pourquoi maintenant : l'objectif est qu'un chat pilote le
# harness d'une machine cliente. Or on ne pilote pas une machine qui n'a pas de
# nom — et aujourd'hui aucune n'en a. Cette sonde mesure exactement ça, sur
# chaque machine : ce qu'elle répond d'elle-même, et si le graphe global la
# connaît.
#
# Elle sert de JALON : le jour où l'auto-déclaration existe, la colonne
# « se nomme ? » passe de non à oui sans qu'on touche au script. Tant qu'elle
# dit non, la fonctionnalité n'est pas là — c'est vérifiable, pas déclaratif.
#
# Usage : bash scripts/flotte-auto-connaissance.sh [hote-du-graphe-global]
#
# Aucune écriture, aucune modification : que des lectures et des sondes.
set -u

GRAPHE="${1:-192.168.1.103}"
STORE_GLOBAL="/home/triviere/.local/share/obrain-harness/data/runtime-dev.obrain"
OBRAIN_LOCAL="$(cd "$(dirname "$0")/.." && pwd)/target/release/obrain"

# nom:ip — les machines connues. n1 refuse la clé, dgx/m4 n'ont pas de SSH :
# c'est justement ce que la sonde doit rendre visible plutôt que supposer.
MACHINES=("n1:192.168.1.101" "n2:192.168.1.102" "n3:192.168.1.103" "dgx:192.168.1.107" "m4:192.168.1.54")

tcp_ouvert() { timeout 2 bash -c "echo > /dev/tcp/$1/$2" 2>/dev/null; }

ssh_b() { timeout 25 ssh -o BatchMode=yes -o StrictHostKeyChecking=no \
                    -o ConnectTimeout=5 "triviere@$1" "$2" 2>/dev/null; }

printf '%-5s %-16s %-6s %-6s %-24s %-10s %s\n' \
       MACHINE IP SSH HTTP80 "DECLARE DE LUI-MEME" "SE NOMME ?" "STORE / HARNESS"
printf '%s\n' "--------------------------------------------------------------------------------------------------"

for m in "${MACHINES[@]}"; do
  nom="${m%%:*}"; ip="${m##*:}"
  ssh="ferme"; [ "$(tcp_ouvert "$ip" 22 && echo o)" = "o" ] && ssh="ouvert"
  http="-"; code=$(curl -s -m 4 -o /dev/null -w '%{http_code}' "http://$ip/" 2>/dev/null); [ -n "$code" ] && http="$code"

  declare="—"; se_nomme="—"; detail="—"

  # 1) Le harness en marche dit-il quelque chose de lui-même ?
  h=$(curl -s -m 4 "http://$ip:8090/api/harness/health" 2>/dev/null)
  if [ -n "$h" ]; then
    declare=$(printf '%s' "$h" | tr -d '\n' | cut -c1-24)
    # « se nomme » = le corps contient le nom de la machine, ou un champ
    # d'identité autre que la version constante et le chemin du store.
    case "$h" in
      *"$nom"*) se_nomme="OUI" ;;
      *)        se_nomme="non" ;;
    esac
    detail="$h"
  fi

  # 2) Pas de SSH : on ne peut rien affirmer de plus, et on le DIT.
  if [ "$ssh" = "ouvert" ]; then
    if ssh_b "$ip" 'true'; then
      b='~/.local/share/obrain-harness/bin/obrain'
      existe=$(ssh_b "$ip" "test -x $b && echo oui || echo non")
      store=$(ssh_b "$ip" 'find ~/.local/share/obrain-harness -maxdepth 2 -name "substrate.meta" 2>/dev/null | wc -l')
      tourne=$(ssh_b "$ip" 'pgrep -c -f "obrain (viz|harness)-serve" 2>/dev/null || echo 0')
      # Le binaire expose-t-il seulement une notion d'identité de machine ?
      idflag=$(ssh_b "$ip" "$b harness-serve --help 2>&1 | grep -ciE 'machine|hote|site|identite|nom'")
      detail="binaire=${existe} stores=${store:-?} services=${tourne:-?} option-identite=${idflag:-?}"
      [ "$h" = "" ] && se_nomme="non (aucun harness en marche)"
    else
      detail="SSH ouvert mais CLE REFUSEE — non inventoriee"
    fi
  fi

  printf '%-5s %-16s %-6s %-6s %-24s %-10s %s\n' "$nom" "$ip" "$ssh" "$http" "$declare" "$se_nomme" "$detail"
done

# ── LE GRAPHE GLOBAL CONNAIT-IL SES MACHINES ? ───────────────────────────────
# C'est la question de fond : un chat ne peut pas adresser une machine que le
# graphe ne nomme pas. On ne cherche pas dans les qn une intention, on compte.
echo
if [ -x "$OBRAIN_LOCAL" ]; then
  n=$(timeout 60 "$OBRAIN_LOCAL" query "$STORE_GLOBAL" --read-only \
        "MATCH (n) WHERE n.qualified_name CONTAINS 'machine:' OR n.qualified_name CONTAINS 'harness:' RETURN count(n) AS c" 2>/dev/null \
        | grep -oE '"[0-9]+"' | head -1 | tr -d '"')
  echo "Graphe global ($GRAPHE) — nœuds nommant une machine ou un harness : ${n:-0}"
  [ "${n:-0}" = "0" ] && echo "  → AUCUNE. La flotte n'existe pas dans le graphe : rien a adresser, rien a autoriser, rien a auditer par machine."
else
  echo "binaire local introuvable ($OBRAIN_LOCAL) — question au graphe non posee"
fi
