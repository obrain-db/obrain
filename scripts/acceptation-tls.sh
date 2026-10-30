#!/usr/bin/env bash
# ── PORTE D'ACCEPTATION DE L'ÉTAPE 0 : LE TRANSPORT CHIFFRÉ ─────────────────
#
# Répond, outils en main, à la question que l'ADR du 2026-09-15 pose en premier :
#
#   > un `tcpdump -A` sur une requête `/api/query` montre-t-il le jeton, la
#   > requête ou le résultat ?
#
# Pourquoi une porte, et pas seulement des tests Rust. Une assertion Rust est
# écrite par la même main que le code qu'elle juge. Ici le **client** est `curl`,
# la **négociation** est lue par `openssl s_client`, et la **vérification de la
# capture** est faite par `grep` — trois outils qui ne savent rien de ce dépôt.
#
# Ce que cette porte ne peut pas faire, et qui est écrit ici pour ne pas être
# vendu : `tcpdump` demande des droits que cet utilisateur n'a pas. La capture
# est donc produite par le relais de `examples/serveur_tls.rs`, qui enregistre
# tous les octets passant dans les deux sens. C'est le même artefact qu'un
# `tcpdump -w`, produit autrement ; et la vérification, elle, reste `grep`.
#
# Le témoin, sans lequel rien ne serait prouvé : le même montage, devant un
# serveur **en clair** (`--clair`), DOIT retrouver le jeton. Si le témoin cesse
# de voir, c'est la capture qui est cassée — et la porte le dit.
#
# Rien n'est laissé derrière : tout vit dans un dossier temporaire supprimé à la
# sortie, les ports sont rendus, aucun service de l'utilisateur n'est touché.
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

DONNEES="$REPO/crates/obrain-tls/tests/donnees"
AUTORITE="$DONNEES/ca.pem"
CERTIFICAT="$DONNEES/cert-de-test.pem"
CLE="$DONNEES/cle-de-test.pem"

JETON="jeton-porteur-3f9a2b7c-a-ne-jamais-traverser-le-reseau"
SECRET="FR76-LE-SECRET-DU-CLIENT-8c1d"

TRAVAIL="$(mktemp -d)"
PIDS=()
ok=0
ko=0
echecs=()

nettoyer() {
    for pid in "${PIDS[@]:-}"; do
        kill "$pid" 2>/dev/null
    done
    wait 2>/dev/null
    rm -rf "$TRAVAIL"
}
trap nettoyer EXIT

verifier() {
    local libelle="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "  ✅ $libelle"
        ok=$((ok + 1))
    else
        echo "  ❌ $libelle"
        ko=$((ko + 1))
        echecs+=("$libelle")
    fi
}

refuser() {
    local libelle="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "  ❌ $libelle (aurait dû échouer)"
        ko=$((ko + 1))
        echecs+=("$libelle")
    else
        echo "  ✅ $libelle"
        ok=$((ok + 1))
    fi
}

# Attend qu'un fichier apparaisse et ne soit pas vide.
attendre_port() {
    local fichier="$1" tours=0
    while [ ! -s "$fichier" ] && [ "$tours" -lt 100 ]; do
        sleep 0.1
        tours=$((tours + 1))
    done
    [ -s "$fichier" ]
}

echo "── 1. Le noyau chiffré compile, et ses tests passent ────────────────────"
if ! cargo build -p obrain-tls --offline --example serveur_tls \
        >"$TRAVAIL/build.log" 2>&1; then
    echo "  ❌ le noyau TLS ne compile pas :"
    tail -20 "$TRAVAIL/build.log"
    exit 1
fi
if grep -qE '^(warning|error)' "$TRAVAIL/build.log"; then
    echo "  ❌ le build a produit un avertissement :"
    grep -E '^(warning|error)' "$TRAVAIL/build.log" | head -5
    ko=$((ko + 1))
    echecs+=("build sans avertissement")
else
    echo "  ✅ le noyau TLS compile sans un avertissement"
    ok=$((ok + 1))
fi
verifier "les 12 critères de la porte TLS (Rust) passent" \
    cargo test -p obrain-tls --offline --quiet

SERVEUR="$REPO/target/debug/examples/serveur_tls"
if [ ! -x "$SERVEUR" ]; then
    echo "  ❌ l'exemple serveur_tls n'a pas été produit"
    exit 1
fi

echo
echo "── 2. Le tunnel, et ce que le fil en montre ────────────────────────────"
"$SERVEUR" servir --port 0 --port-ecrit "$TRAVAIL/port-serveur" \
    --cert "$CERTIFICAT" --cle "$CLE" --jeton "$JETON" >"$TRAVAIL/serveur.log" 2>&1 &
PIDS+=($!)
attendre_port "$TRAVAIL/port-serveur" || { echo "  ❌ le serveur n'a pas démarré"; cat "$TRAVAIL/serveur.log"; exit 1; }
PORT_SERVEUR="$(cat "$TRAVAIL/port-serveur")"
echo "  ℹ️  serveur chiffré sur 127.0.0.1:$PORT_SERVEUR"

URL_QUERY="https://localhost:$PORT_SERVEUR/api/query?q=$SECRET"

# Le client est `curl`, l'autorité est celle des données de test.
verifier "une requête HTTPS aboutit (curl, autorité de test)" \
    curl --silent --show-error --fail --cacert "$AUTORITE" \
    --header "Authorization: Bearer $JETON" "$URL_QUERY"

verifier "la réponse porte le résultat du calcul" \
    bash -c "curl -s --cacert '$AUTORITE' -H 'Authorization: Bearer $JETON' '$URL_QUERY' | grep -q RESULTAT-DU-CALCUL"

verifier "le protocole négocié est TLS 1.3" \
    bash -c "echo | openssl s_client -connect 127.0.0.1:$PORT_SERVEUR -CAfile '$AUTORITE' 2>/dev/null | grep -q 'TLSv1.3'"

# ── La moitié « vérifiable par la partie qui la subit » ────────────────────
refuser "un client qui ne connaît pas l'autorité est refusé" \
    curl --silent --show-error --fail "https://localhost:$PORT_SERVEUR/api/info"

# ── Aucune descente en clair ───────────────────────────────────────────────
refuser "du HTTP en clair sur le port chiffré n'obtient rien" \
    bash -c "curl -s --max-time 5 'http://127.0.0.1:$PORT_SERVEUR/api/info' | grep -q 'HTTP/1.1 200'"

# ── Le jeton reste exigé DANS le tunnel ────────────────────────────────────
verifier "sans jeton, le tunnel rend 401" \
    bash -c "[ \"\$(curl -s -o /dev/null -w '%{http_code}' --cacert '$AUTORITE' 'https://localhost:$PORT_SERVEUR/api/query?q=$SECRET')\" = '401' ]"

echo
echo "── 3. Sur le fil : rien du calcul ──────────────────────────────────────"
"$SERVEUR" relais --ecoute 0 --vers "$PORT_SERVEUR" \
    --capture "$TRAVAIL/fil-chiffre.bin" --port-ecrit "$TRAVAIL/port-relais" \
    >"$TRAVAIL/relais.log" 2>&1 &
PIDS+=($!)
attendre_port "$TRAVAIL/port-relais" || { echo "  ❌ le relais n'a pas démarré"; exit 1; }
PORT_RELAIS="$(cat "$TRAVAIL/port-relais")"

curl --silent --show-error --fail --cacert "$AUTORITE" \
    --header "Authorization: Bearer $JETON" \
    "https://localhost:$PORT_RELAIS/api/query?q=$SECRET" >"$TRAVAIL/reponse.json" 2>&1
sleep 0.5

TAILLE="$(wc -c <"$TRAVAIL/fil-chiffre.bin" 2>/dev/null || echo 0)"
echo "  ℹ️  capture : $TAILLE octets dans les deux sens"
verifier "la capture n'est pas vide (sinon elle ne prouverait rien)" \
    test "$TAILLE" -gt 200
verifier "le premier octet est un enregistrement de poignée de main TLS (0x16)" \
    bash -c "[ \"\$(od -An -tx1 -N1 '$TRAVAIL/fil-chiffre.bin' | tr -d ' ')\" = '16' ]"
refuser "grep ne trouve pas le secret du calcul" \
    grep -q "$SECRET" "$TRAVAIL/fil-chiffre.bin"
refuser "grep ne trouve pas le jeton porteur" \
    grep -q "$JETON" "$TRAVAIL/fil-chiffre.bin"
refuser "grep ne trouve pas le résultat" \
    grep -q "RESULTAT-DU-CALCUL" "$TRAVAIL/fil-chiffre.bin"
refuser "grep ne trouve pas l'en-tête d'autorisation" \
    grep -q "Authorization" "$TRAVAIL/fil-chiffre.bin"

echo
echo "── 4. Le témoin : la capture SAIT voir un secret en clair ──────────────"
"$SERVEUR" servir --port 0 --clair --port-ecrit "$TRAVAIL/port-clair" \
    --jeton "$JETON" >"$TRAVAIL/clair.log" 2>&1 &
PIDS+=($!)
attendre_port "$TRAVAIL/port-clair" || { echo "  ❌ le témoin n'a pas démarré"; exit 1; }
PORT_CLAIR="$(cat "$TRAVAIL/port-clair")"

"$SERVEUR" relais --ecoute 0 --vers "$PORT_CLAIR" \
    --capture "$TRAVAIL/fil-clair.bin" --port-ecrit "$TRAVAIL/port-relais-clair" \
    >"$TRAVAIL/relais-clair.log" 2>&1 &
PIDS+=($!)
attendre_port "$TRAVAIL/port-relais-clair" || { echo "  ❌ le relais témoin n'a pas démarré"; exit 1; }
PORT_RELAIS_CLAIR="$(cat "$TRAVAIL/port-relais-clair")"

curl --silent --show-error --fail -H "Authorization: Bearer $JETON" \
    "http://127.0.0.1:$PORT_RELAIS_CLAIR/api/query?q=$SECRET" >/dev/null 2>&1
sleep 0.5

verifier "le témoin VOIT le jeton en clair (la capture a la puissance de détecter)" \
    grep -q "$JETON" "$TRAVAIL/fil-clair.bin"
verifier "le témoin VOIT le secret en clair" \
    grep -q "$SECRET" "$TRAVAIL/fil-clair.bin"

echo
echo "────────────────────────────────────────────────────────────────────────"
echo "porte TLS (étape 0) : $ok vérifications passées, $ko échecs"
if [ "$ko" -ne 0 ]; then
    echo "échecs :"
    for e in "${echecs[@]}"; do echo "  - $e"; done
    echo "════ ÉCHEC ════"
    exit 1
fi
echo "════ TOUT PASSE ════"
echo "Rappel de ce que cette porte NE prouve PAS : que le clair ne soit pas lu"
echo "pendant le calcul sur la machine de l'opérateur (§5 et §7 de l'ADR)."
