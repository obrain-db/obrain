#!/usr/bin/env bash
# ── PORTE D'ACCEPTATION DE LA CONFIDENTIALITÉ DU CALCUL ─────────────────────
#
# Un seul point d'entrée qui répond à : « les calculs d'un utilisateur peuvent-ils
# être écoutés par quelqu'un d'autre — y compris par celui qui exécute
# l'engine ? » — sans moi, et sans la main.
#
# Pourquoi cette porte existe. L'ADR du 2026-09-15 a posé la garantie, mais une
# garantie qui n'est pas exécutée est une intention. Cette porte rend chaque
# critère **falsifiable par un outil indépendant du code testé** : le littéral
# cherché dans le fichier scellé est cherché par `grep`, pas par une assertion
# Rust qui pourrait se tromper avec le code qu'elle teste.
#
# Ce qu'elle vérifie, dans l'ordre :
#   1. le noyau compile sans un avertissement, et ses tests passent ;
#   2. un graphe scellé sur le disque ne rend AUCUN littéral — `grep` ne trouve
#      rien, et `hexdump` ne montre que de l'indistinct ;
#   3. le tour complet marche : sceller, desceller, retrouver l'octet près ;
#   4. une phrase fausse ne descelle RIEN ;
#   5. un octet modifié n'importe où est DÉTECTÉ ;
#   6. le fichier de clé est en 0600, et un fichier trop permissif est REFUSÉ ;
#   7. deux charges utiles de tailles différentes ont la MÊME longueur scellée ;
#   8. tous les droits IAM ne donnent PAS le contenu (le test de frontière) ;
#   9. le noyau ne contient aucun code non sûr, et l'interdiction est déclarée.
#
# Ce qu'elle ne vérifie pas, et qui est écrit ici pour ne pas être vendu : que
# l'opérateur ne lise pas le clair PENDANT le calcul sur sa propre machine.
# Aucun outil logiciel ne peut le prouver — c'est l'objet des §5 et §7 de l'ADR.
#
# Usage :
#   scripts/acceptation-coffre.sh
#
# Rien n'est laissé derrière : tout vit dans un dossier temporaire, supprimé à
# la sortie. Aucun port n'est ouvert, aucun service de l'utilisateur n'est
# touché.
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

PHRASE="phrase-du-client-que-l-engine-ne-connait-pas"
ESPACE="espace-du-client"
LITTERAL="RACINE-BANQUE-ALPHA-FR76-SECRET"

TRAVAIL="$(mktemp -d)"
trap 'rm -rf "$TRAVAIL"' EXIT

ok=0
ko=0
echecs=()

verifier() { # verifier <nom> <commande...>
    local nom="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        printf '  ✅ %s\n' "$nom"
        ok=$((ok + 1))
    else
        printf '  ❌ %s\n' "$nom"
        ko=$((ko + 1))
        echecs+=("$nom")
    fi
}

refuser() { # refuser <nom> <commande...> — la commande DOIT échouer
    local nom="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        printf '  ❌ %s (a réussi alors qu’il devait refuser)\n' "$nom"
        ko=$((ko + 1))
        echecs+=("$nom")
    else
        printf '  ✅ %s\n' "$nom"
        ok=$((ok + 1))
    fi
}

echo "── 1. Le noyau compile et ses tests passent ─────────────────────────────"
if ! cargo build -p obrain-coffre --offline --example sceller \
        --manifest-path "$REPO/Cargo.toml" >"$TRAVAIL/build.log" 2>&1; then
    echo "  ❌ le noyau ne compile pas :"
    tail -30 "$TRAVAIL/build.log"
    exit 1
fi
if grep -qE '^(warning|error)' "$TRAVAIL/build.log"; then
    echo "  ❌ le build a produit un avertissement :"
    grep -E '^(warning|error)' "$TRAVAIL/build.log" | head -10
    ko=$((ko + 1))
    echecs+=("build sans avertissement")
else
  echo "  ✅ le noyau compile sans un avertissement"
  ok=$((ok + 1))
fi

SCELLER="$REPO/target/debug/examples/sceller"
if [ ! -x "$SCELLER" ]; then
    echo "  ❌ l'exemple sceller n'a pas été produit"
    exit 1
fi

verifier "les 71 tests du noyau passent" \
    cargo test -p obrain-coffre --offline --lib --quiet
verifier "les 11 critères de la porte (Rust) passent" \
    cargo test -p obrain-coffre --offline --test porte_confidentialite --quiet

echo
echo "── 2. Sur le disque, aucun littéral ne se lit ──────────────────────────"
CLAIR="$TRAVAIL/graphe.obrain"
SCELLE="$TRAVAIL/graphe.scelle"
CLE="$TRAVAIL/espace.cle"
{
    echo "$LITTERAL"
    for i in $(seq 0 199); do
        echo "  (compte-$i:Compte {iban: \"FR76-SECRET-$(printf '%04d' "$i")\"})"
    done
} >"$CLAIR"

verifier "le graphe est scellé (phrase, espace, scellé, clé)" \
    "$SCELLER" sceller "$PHRASE" "$ESPACE" "$CLAIR" "$SCELLE" "$CLE"

# ⚠️ LE critère de l'ADR, vérifié par grep — un outil qui ignore tout du code.
refuser "grep ne trouve pas le littéral « $LITTERAL »" \
    grep -q "$LITTERAL" "$SCELLE"
refuser "grep ne trouve aucun IBAN" grep -q "FR76-SECRET" "$SCELLE"
refuser "grep ne trouve pas le nom de l'espace" grep -q "$ESPACE" "$SCELLE"
refuser "grep ne trouve pas la phrase" grep -q "$PHRASE" "$SCELLE"

echo "  ℹ️  ce que le porteur voit du début du scellé :"
hexdump -C "$SCELLE" | head -3 | sed 's/^/      /'

echo
echo "── 3. Le tour complet, et ce qui doit échouer ──────────────────────────"
RENDU="$TRAVAIL/rendu.obrain"
verifier "le client descelle son propre contenu" \
    "$SCELLER" desceller "$PHRASE" "$CLE" "$SCELLE" "$RENDU"
verifier "le contenu descellé est identique à l'octet près" \
    cmp -s "$CLAIR" "$RENDU"

refuser "une phrase fausse ne descelle rien" \
    "$SCELLER" desceller "phrase-fausse" "$CLE" "$SCELLE" "$TRAVAIL/jamais.obrain"

# Un octet modifié au milieu du chiffré : la substitution doit être détectée.
FALSIFIE="$TRAVAIL/falsifie.scelle"
cp "$SCELLE" "$FALSIFIE"
POSITION=$(( $(stat -c%s "$SCELLE") / 2 ))
printf '\x01' | dd of="$FALSIFIE" bs=1 seek="$POSITION" count=1 conv=notrunc 2>/dev/null
refuser "un octet modifié est détecté" \
    "$SCELLER" desceller "$PHRASE" "$CLE" "$FALSIFIE" "$TRAVAIL/falsifie.obrain"

# Un octet modifié dans l'EN-TÊTE (l'identifiant du coffre) : détecté aussi.
ENTETE_FALSIFIE="$TRAVAIL/entete.scelle"
cp "$SCELLE" "$ENTETE_FALSIFIE"
printf '\xff' | dd of="$ENTETE_FALSIFIE" bs=1 seek=60 count=1 conv=notrunc 2>/dev/null
refuser "un octet modifié dans l'en-tête est détecté" \
    "$SCELLER" desceller "$PHRASE" "$CLE" "$ENTETE_FALSIFIE" "$TRAVAIL/entete.obrain"

# Troncature.
TRONQUE="$TRAVAIL/tronque.scelle"
head -c $(( $(stat -c%s "$SCELLE") - 32 )) "$SCELLE" >"$TRONQUE"
refuser "une troncature est détectée" \
    "$SCELLER" desceller "$PHRASE" "$CLE" "$TRONQUE" "$TRAVAIL/tronque.obrain"

echo
echo "── 4. La clé, et ses permissions ───────────────────────────────────────"
MODE=$(stat -c '%a' "$CLE")
if [ "$MODE" = "600" ]; then
    echo "  ✅ le fichier de clé est en 0600"
    ok=$((ok + 1))
else
    echo "  ❌ le fichier de clé est en $MODE, attendu 600"
    ko=$((ko + 1))
    echecs+=("permissions du fichier de clé")
fi

PERMISSIF="$TRAVAIL/permissif.cle"
cp "$CLE" "$PERMISSIF"
chmod 644 "$PERMISSIF"
refuser "un fichier de clé lisible par d'autres est refusé" \
    "$SCELLER" desceller "$PHRASE" "$PERMISSIF" "$SCELLE" "$TRAVAIL/permissif.obrain"

refuser "un fichier de clé absent est une impossibilité, pas un refus silencieux" \
    "$SCELLER" desceller "$PHRASE" "$TRAVAIL/jamais-posee.cle" "$SCELLE" "$TRAVAIL/absent.obrain"

echo
echo "── 5. G3 : deux calculs différents, la même taille ─────────────────────"
ENV_A="$TRAVAIL/a.env"
ENV_B="$TRAVAIL/b.env"
verifier "enveloppe d'une requête courte" \
    "$SCELLER" enveloppe "$ESPACE" "RETURN 1" "$ENV_A"
verifier "enveloppe d'une requête longue et toute différente" \
    "$SCELLER" enveloppe "$ESPACE" \
    "MATCH (n:Compte) WHERE n.iban = 'FR76-SECRET-0042' RETURN n.solde" "$ENV_B"

TAILLE_A=$(stat -c%s "$ENV_A")
TAILLE_B=$(stat -c%s "$ENV_B")
if [ "$TAILLE_A" = "$TAILLE_B" ]; then
    echo "  ✅ tailles identiques ($TAILLE_A octets) malgré des contenus différents"
    ok=$((ok + 1))
else
    echo "  ❌ tailles différentes : $TAILLE_A contre $TAILLE_B"
    ko=$((ko + 1))
    echecs+=("tailles d'enveloppe")
fi
refuser "l'action d'une requête ne se lit pas dans l'enveloppe" \
    grep -q "RETURN" "$ENV_B"

echo
echo "── 6. Le droit ne remplace pas la clé (frontière IAM) ──────────────────"
verifier "tous les droits IAM ne donnent pas le contenu" \
    cargo test -p obrain-iam --offline --test la_cle_prime_sur_le_droit --quiet
verifier "l'IAM n'a pas régressé (139 tests)" \
    cargo test -p obrain-iam --offline --lib --quiet

echo
echo "── 7. Le noyau lui-même ────────────────────────────────────────────────"
verifier "forbid(unsafe_code) est déclaré dans lib.rs" \
    grep -q "forbid(unsafe_code)" "$REPO/crates/obrain-coffre/src/lib.rs"
refuser "aucun bloc de code non sûr dans le noyau" \
    grep -rnE "unsafe (fn|impl|trait|\{)" "$REPO/crates/obrain-coffre/src/"

echo
echo "────────────────────────────────────────────────────────────────────────"
printf 'porte de confidentialité : %d vérifications passées, %d échecs\n' "$ok" "$ko"
if [ "$ko" -ne 0 ]; then
    printf 'échecs :\n'
    for e in "${echecs[@]}"; do printf '  - %s\n' "$e"; done
    exit 1
fi
echo "════ TOUT PASSE ════"
echo "Rappel de ce que cette porte NE prouve PAS : que le clair ne soit pas lu"
echo "pendant le calcul sur la machine de l'opérateur. Le déscellement doit avoir"
echo "lieu chez qui a le droit de lire (§7 de l'ADR)."
