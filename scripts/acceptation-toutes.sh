#!/usr/bin/env bash
# Lance les portes d'acceptation du dépôt, une par une, contre le binaire
# fraîchement construit, et rend un verdict par porte.
#
# POURQUOI CE FICHIER EXISTE : chaque porte est autonome (elle crée ses stores
# dans /tmp, prend son port, lance son propre `viz serve`), mais les dix lancer
# à la main demande de retenir dix ports, dix prérequis et dix codes de sortie —
# et une campagne à la main se termine toujours par « je crois que ça passait ».
#
# Aucune porte ne touche le service de production : `acceptation-8432.sh` part
# sur l'instance isolée 8433 tant qu'on ne lui passe pas `--vif 8432`.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

BIN="${OBRAIN_BIN:-$REPO/target/release/obrain}"
PORTES_DIR="${PORTES_DIR:-/tmp/portes-$(date +%Y%m%d-%H%M%S)}"
DELAI="${DELAI:-1800}"

mkdir -p "$PORTES_DIR"

if [ ! -x "$BIN" ]; then
    echo "binaire absent : $BIN"
    echo "  construire d'abord : cargo build --release -p obrain-cli"
    exit 2
fi

# Les portes qui compilent (coffre, journal, tls) payent le build des crates
# qu'elles jugent ; on leur laisse le temps. L'ordre va du moins coûteux au plus
# coûteux, pour qu'un échec précoce ne fasse pas attendre une heure.
PORTES=(
    acceptation-coffre.sh
    acceptation-journal.sh
    acceptation-tls.sh
    acceptation-iam.sh
    acceptation-selfs-catalogue.sh
    acceptation-self-par-compte.sh
    acceptation-chat-self.sh
    acceptation-resonance.sh
    acceptation-monde-utilisateur.sh
    acceptation-8432.sh
)

echo "binaire   : $BIN"
echo "empreinte : $(md5sum "$BIN" | cut -d' ' -f1)"
echo "journaux  : $PORTES_DIR"
echo

verdicts=()
total_ok=0
total_ko=0

for porte in "${PORTES[@]}"; do
    chemin="scripts/$porte"
    if [ ! -x "$chemin" ]; then
        printf '%-36s ABSENTE\n' "$porte"
        verdicts+=("$porte|ABSENTE|0")
        total_ko=$((total_ko + 1))
        continue
    fi
    journal="$PORTES_DIR/${porte%.sh}.log"
    debut=$(date +%s)
    OBRAIN_BIN="$BIN" timeout "$DELAI" bash "$chemin" >"$journal" 2>&1
    code=$?
    duree=$(( $(date +%s) - debut ))
    # `grep -c` IMPRIME 0 et sort en 1 quand il ne trouve rien : le `|| echo 0`
    # ajoutait donc un SECOND zéro (« 0\n0 »). On lit le compte, c'est tout.
    reussies=$(grep -c '✅' "$journal" 2>/dev/null)
    echouees=$(grep -c '❌' "$journal" 2>/dev/null)
    if [ "$code" -eq 0 ]; then
        printf '%-36s PASS  (%ss, %s ✅, %s ❌)\n' "$porte" "$duree" "$reussies" "$echouees"
        total_ok=$((total_ok + 1))
    elif [ "$code" -eq 124 ]; then
        printf '%-36s DÉLAI (%ss)\n' "$porte" "$duree"
        total_ko=$((total_ko + 1))
    else
        printf '%-36s ÉCHEC (code %s, %ss, %s ✅, %s ❌)\n' "$porte" "$code" "$duree" "$reussies" "$echouees"
        total_ko=$((total_ko + 1))
    fi
    verdicts+=("$porte|$code|$duree")
done

echo
echo "── verdict ────────────────────────────────────────────────────────────"
printf 'portes passées : %s\n' "$total_ok"
printf 'portes en échec : %s\n' "$total_ko"
echo
echo "ce qui a échoué, et où le lire :"
for v in "${verdicts[@]}"; do
    IFS='|' read -r nom code duree <<<"$v"
    [ "$code" = "0" ] && continue
    echo "  · $nom (code $code) → $PORTES_DIR/${nom%.sh}.log"
    grep -m3 '❌' "$PORTES_DIR/${nom%.sh}.log" 2>/dev/null | sed 's/^/      /'
done

[ "$total_ko" -eq 0 ]
