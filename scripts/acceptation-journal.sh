#!/usr/bin/env bash
# ── PORTE D'ACCEPTATION DU JOURNAL DE CALCUL (ÉTAPE 4 DE L'ADR) ─────────────
#
# L'ADR, §4.1, réserve n° 1 — le constat qui a fait écrire cette crate :
#
#   « `model.rs` décrit ses entrées d'audit comme des "immutable audit trail
#     entries". Or il n'y a NI chaînage par hachage, NI signature, et le magasin
#     appartient à l'opérateur. Il peut donc supprimer ou réécrire un événement
#     sans que rien ne le signale. »
#
# Cette porte vérifie, hors de tout test Rust, que ce n'est plus vrai — et avec
# des outils qui ne savent rien de ce dépôt :
#
#   * l'altération est faite par `dd` (elle ne passe par aucune API) ;
#   * le contenu est cherché par `grep` (et non par une assertion qui pourrait
#     se tromper avec le code qu'elle juge) ;
#   * l'arithmétique du format est vérifiée par `stat` et le shell.
#
# Ce qu'elle vérifie, dans l'ordre :
#   1. la crate compile sans un avertissement, et ses 16 tests passent ;
#   2. un journal s'écrit sur le disque ;
#   3. `grep` ne trouve AUCUNE charge utile dans le fichier — le journal ne
#      conserve que des empreintes ;
#   4. le journal se relit et ses signatures se vérifient ;
#   5. la taille du fichier est exactement celle qu'annonce le format ;
#   6. un octet altéré par `dd` rend la relecture REFUSÉE ;
#   7. une signature vérifiée contre une AUTRE clé est refusée ;
#   8. tronquer le fichier SE CHARGE (sa chaîne est intacte, elle est juste plus
#      courte) et n'est démasqué que par la tête conservée par le client ;
#   9. deux journaux différents n'ont pas la même tête.
#
# Ce qu'elle ne vérifie PAS, et qui est écrit ici pour ne pas être vendu :
#
#   * **la complétude** : un opérateur qui n'inscrit rien produit un journal
#     valide. Ce qui le prend, c'est un client qui attend un reçu — c'est le
#     test `omettre_un_calcul_est_detecte_par_les_recus_du_client`, côté Rust ;
#   * **l'honnêteté du calcul** : le journal prouve qu'une entrée existe à une
#     place, pas que le calcul annoncé a été exécuté (seconde moitié de
#     l'étape 4, mesure de code attestée) ;
#   * **la confidentialité du journal lui-même** : il ne dit rien du contenu
#     parce qu'il n'en garde que l'empreinte, mais l'empreinte d'une charge
#     devinable reste devinable.
#
# Usage :
#   scripts/acceptation-journal.sh
#
# Rien n'est laissé derrière : tout vit dans un dossier temporaire supprimé à la
# sortie, et aucun service de l'utilisateur n'est touché.
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

TRAVAIL="$(mktemp -d)"
trap 'rm -rf "$TRAVAIL"' EXIT

CLE="$(printf '42%.0s' $(seq 1 32))"        # 32 octets à 0x42
AUTRE_CLE="$(printf '07%.0s' $(seq 1 32))"  # 32 octets à 0x07

SECRET="RACINE-BANQUE-ALPHA-FR76-SECRET-8c1d55aa"
IDENTIFIANT="espace-alpha"

JOURNAL="$TRAVAIL/journal.obj"
ok=0
ko=0
echecs=()

verifier() { # nom, commande...
    local nom="$1"; shift
    if "$@" >"$TRAVAIL/sortie" 2>&1; then
        echo "  ✅ $nom"; ok=$((ok + 1))
    else
        echo "  ❌ $nom"; tail -5 "$TRAVAIL/sortie" | sed 's/^/       /'
        ko=$((ko + 1)); echecs+=("$nom")
    fi
}

verifier_echec() { # nom, commande... — la commande DOIT échouer
    local nom="$1"; shift
    if "$@" >"$TRAVAIL/sortie" 2>&1; then
        echo "  ❌ $nom — a réussi alors qu'il devait échouer"
        ko=$((ko + 1)); echecs+=("$nom")
    else
        echo "  ✅ $nom"; ok=$((ok + 1))
    fi
}

echo "── 1. La crate compile et ses tests passent ─────────────────────────────"
if ! cargo build -p obrain-journal --offline --example journal >"$TRAVAIL/build.log" 2>&1; then
    echo "  ❌ la crate ne compile pas :"
    tail -30 "$TRAVAIL/build.log"
    exit 1
fi
if grep -qE '^(warning|error)' "$TRAVAIL/build.log"; then
    echo "  ❌ le build a produit un avertissement :"
    grep -E '^(warning|error)' "$TRAVAIL/build.log" | head -10
    ko=$((ko + 1)); echecs+=("build sans avertissement")
else
    echo "  ✅ la crate compile sans un avertissement"; ok=$((ok + 1))
fi

OUTIL="$REPO/target/debug/examples/journal"
if [ ! -x "$OUTIL" ]; then
    echo "  ❌ l'exemple journal n'a pas été produit"
    exit 1
fi

verifier "les 16 tests du journal passent" \
    cargo test -p obrain-journal --offline --quiet

echo
echo "── 2. Un journal s'écrit, et ne dit rien de ce qu'il enregistre ─────────"
if ! "$OUTIL" ecrire "$JOURNAL" "$IDENTIFIANT" "$CLE" \
        "$SECRET" "calcul-deuxieme" "calcul-troisieme" >"$TRAVAIL/ecriture.txt" 2>&1; then
    echo "  ❌ l'écriture a échoué :"
    tail -5 "$TRAVAIL/ecriture.txt" | sed 's/^/       /'
    exit 1
fi
echo "  ✅ le journal est écrit ($(grep '^OCTETS=' "$TRAVAIL/ecriture.txt" | cut -d= -f2) octets, $(grep '^ENTREES=' "$TRAVAIL/ecriture.txt" | cut -d= -f2) entrées)"
ok=$((ok + 1))

# Le point qui compte : `grep`, outil indépendant, ne trouve RIEN du contenu.
if grep -q -- "$SECRET" "$JOURNAL"; then
    echo "  ❌ la charge utile se lit dans le journal !"
    ko=$((ko + 1)); echecs+=("le contenu ne doit pas être dans le journal")
else
    echo "  ✅ grep ne trouve AUCUNE charge utile dans le journal"; ok=$((ok + 1))
fi
if grep -qa "calcul-deuxieme" "$JOURNAL"; then
    echo "  ❌ un libellé de calcul se lit dans le journal !"
    ko=$((ko + 1)); echecs+=("aucun libellé dans le journal")
else
    echo "  ✅ grep ne trouve aucun libellé de calcul"; ok=$((ok + 1))
fi

echo
echo "── 3. Le journal se relit, et ses signatures tiennent ───────────────────"
verifier "la relecture vérifie chaîne et signatures" \
    "$OUTIL" verifier "$JOURNAL" "$CLE"
TETE_ECRITE="$(grep '^TETE=' "$TRAVAIL/ecriture.txt" | cut -d= -f2)"
TETE_RELUE="$(grep '^TETE=' "$TRAVAIL/sortie" | cut -d= -f2)"
if [ -n "$TETE_ECRITE" ] && [ "$TETE_ECRITE" = "$TETE_RELUE" ]; then
    echo "  ✅ la tête relue est celle qui a été écrite"; ok=$((ok + 1))
else
    echo "  ❌ la tête a changé entre l'écriture et la relecture"
    ko=$((ko + 1)); echecs+=("tête stable")
fi

# L'arithmétique du format, vérifiée par `stat` et le shell : en-tête variable
# (magic 8 + longueur 2 + nom + n 8) puis des entrées à taille fixe de 176.
OCTETS="$(stat -c%s "$JOURNAL")"
ENTREES="$(grep '^ENTREES=' "$TRAVAIL/ecriture.txt" | cut -d= -f2)"
ATTENDU=$((8 + 2 + ${#IDENTIFIANT} + 8 + ENTREES * 176))
if [ "$OCTETS" -eq "$ATTENDU" ]; then
    echo "  ✅ la taille du fichier est exactement celle du format ($OCTETS octets)"
    ok=$((ok + 1))
else
    echo "  ❌ taille inattendue : $OCTETS octets, $ATTENDU attendus"
    ko=$((ko + 1)); echecs+=("taille du format")
fi

echo
echo "── 4. Ce que l'opérateur peut faire, et ce qui le signale ───────────────"
# 4a. Un octet altéré, par `dd` — donc sans passer par aucune API.
cp "$JOURNAL" "$TRAVAIL/altere.obj"
printf '\xff' | dd of="$TRAVAIL/altere.obj" bs=1 seek=$((ATTENDU - 40)) conv=notrunc 2>/dev/null
verifier_echec "un octet altéré (par dd) rend la relecture refusée" \
    "$OUTIL" verifier "$TRAVAIL/altere.obj" "$CLE"

# 4b. Une autre clé : la chaîne peut tenir, la signature ne suit pas.
verifier_echec "les mêmes octets vérifiés contre une AUTRE clé sont refusés" \
    "$OUTIL" verifier "$JOURNAL" "$AUTRE_CLE"

# 4c. La troncature : retirer la dernière entrée ET recoller l'en-tête, ce que
#     ferait un opérateur soigneux.
cp "$JOURNAL" "$TRAVAIL/tronque.obj"
head -c $((ATTENDU - 176)) "$JOURNAL" >"$TRAVAIL/corps"
# Le nombre d'entrées annoncé est recollé à la coupe : sans cela, la troncature
# serait attrapée par une simple incohérence de longueur, et le test ne dirait
# rien de la chaîne.
python3 - "$TRAVAIL/corps" "$TRAVAIL/tronque.obj" "$((ENTREES - 1))" <<'PY'
import struct, sys
corps, sortie, reste = sys.argv[1], sys.argv[2], int(sys.argv[3])
octets = bytearray(open(corps, 'rb').read())
debut = 8 + 2 + int(struct.unpack_from('<H', octets, 8)[0]) + 8
struct.pack_into('<Q', octets, debut - 8, reste)
open(sortie, 'wb').write(bytes(octets))
PY
verifier "un journal tronqué et recollé SE CHARGE (sa chaîne est intacte)" \
    "$OUTIL" verifier "$TRAVAIL/tronque.obj" "$CLE"

# 4d. Ce qui le démasque, et qui ne peut venir que du client : la tête conservée.
verifier_echec "la MÊME relecture, confrontée à la tête conservée, est REFUSÉE" \
    "$OUTIL" verifier "$TRAVAIL/tronque.obj" "$CLE" "$TETE_ECRITE"

# 4e. Et la tête du journal tronqué diffère bien de celle du client.
TETE_TRONQUEE="$("$OUTIL" tete "$TRAVAIL/tronque.obj" 2>/dev/null | cut -d= -f2)"
if [ -n "$TETE_TRONQUEE" ] && [ "$TETE_TRONQUEE" != "$TETE_ECRITE" ]; then
    echo "  ✅ la tête du journal tronqué DIFFÈRE de celle conservée par le client"
    ok=$((ok + 1))
else
    echo "  ❌ la troncature n'a pas changé la tête — la détection ne marcherait pas"
    ko=$((ko + 1)); echecs+=("tête du journal tronqué")
fi

# 4f. Deux journaux différents ne partagent pas leur genèse : un reçu de l'un ne
#     peut pas être versé dans l'autre.
"$OUTIL" ecrire "$TRAVAIL/beta.obj" "espace-beta" "$CLE" "calcul" >"$TRAVAIL/beta.txt" 2>&1
TETE_BETA="$(grep '^TETE=' "$TRAVAIL/beta.txt" | cut -d= -f2)"
if [ -n "$TETE_BETA" ] && [ "$TETE_BETA" != "$TETE_ECRITE" ]; then
    echo "  ✅ deux journaux distincts n'ont pas la même tête"; ok=$((ok + 1))
else
    echo "  ❌ deux journaux distincts partagent une tête"
    ko=$((ko + 1)); echecs+=("têtes distinctes")
fi

echo
echo "────────────────────────────────────────────────────────────────────────"
if [ "$ko" -eq 0 ]; then
    echo "porte du journal : $ok vérifications passées, 0 échec"
    echo "════ TOUT PASSE ════"
else
    echo "porte du journal : $ok passées, $ko ÉCHECS"
    for e in "${echecs[@]}"; do echo "    ✗ $e"; done
fi
echo "Rappel de ce que cette porte NE prouve PAS : la complétude (un opérateur"
echo "qui n'inscrit rien produit un journal valide — c'est le reçu attendu par le"
echo "client qui le prend), ni que le calcul annoncé a été exécuté (mesure de code"
echo "attestée), ni la confidentialité d'une charge devinable."

[ "$ko" -eq 0 ]
