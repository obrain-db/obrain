#!/usr/bin/env bash
# ── LA FLOTTE ENTRE DANS LA BASE DE CHAQUE COMPTE ───────────────────────────
#
# La passe `obrain machines --appliquer` exige l'EXCLUSIF sur la base qu'elle
# écrit. Le moteur, lui, tient ses presets en LECTURE (verrou partagé) : viser la
# base servie ferait donc échouer la passe à chaque passage — et un timer qui
# échoue toujours est un timer qu'on apprend à ignorer. C'est précisément le
# motif de défaillance que ce dépôt refuse (« une garde qui tombe en silence ne
# garde rien », et son jumeau : une garde qui crie toujours ne garde rien non
# plus).
#
# La cible juste est donc la base DE CHAQUE COMPTE — `selfs/<compte>.obrain` —
# que nul service ne tient en exclusif. Chaque compte y apprend QUELLES MACHINES
# SONT LES SIENNES. C'est la décision du 2026-09-16 : la flotte est une affaire de
# compte, pas une vue d'ensemble sur tout le monde
# (docs/ACCEPTATION-FLOTTE-2026-09-16.md §9.2).
#
# Les comptes sont DÉCOUVERTS par le registre des machines, et non par l'annuaire
# IAM — celui-ci est tenu en écriture par le serveur armé, donc illisible hors
# bande. Même principe que `memoire-appliquer-comptes.sh`.
#
# Usage : scripts/machines-appliquer-comptes.sh [--verbeux]
#   OBRAIN_BIN=<binaire>   binaire à utiliser (défaut : celui du déploiement,
#                          puis celui du dépôt)
set -u

VERBEUX=0
[ "${1:-}" = "--verbeux" ] && VERBEUX=1

DATA="${OBRAIN_HARNESS_DATA:-$HOME/.local/share/obrain-harness/data}"
BIN="${OBRAIN_BIN:-}"
if [ -z "$BIN" ]; then
    for c in "$HOME/.local/share/obrain-harness/bin/obrain" \
             "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/target/release/obrain"; do
        [ -x "$c" ] && { BIN="$c"; break; }
    done
fi
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
    echo "machines : binaire INTROUVABLE — posez OBRAIN_BIN=<chemin>" >&2
    exit 2
fi

# ── LA GARDE QUI PARLE ──────────────────────────────────────────────────────
# Un binaire qui ne connaît pas la sous-commande ferait échouer chaque passage en
# silence (le timer ne dit rien, personne ne lit ses journaux). On le dit UNE fois,
# fort, et on sort non-zéro : la panne doit être visible AVANT qu'on l'oublie.
if ! "$BIN" machines --help >/dev/null 2>&1; then
    echo "machines : ce binaire ne connaît pas la sous-commande « machines » —" >&2
    echo "           la flotte ne sera PAS inscrite dans les bases des comptes." >&2
    echo "           binaire : $BIN" >&2
    echo "           Reconstruire : cargo build --release -p obrain-cli" >&2
    exit 3
fi

# Les comptes : ceux que le registre des machines connaît. On lit le champ
# « compte » de ses journaux, sans inverser le hachage du nom de fichier.
CACHE="${OBRAIN_VIZ_CACHE_DIR:-/tmp/obrain-viz-cache}"
J="$CACHE/machines"
[ -d "$J" ] || { echo "machines : aucun registre ($J) — rien à inscrire"; exit 0; }

COMPTES="$(python3 - "$J" <<'PY'
import json, os, sys
comptes = set()
for f in os.listdir(sys.argv[1]):
    if not f.endswith(".jsonl"):
        continue
    try:
        for ligne in open(os.path.join(sys.argv[1], f), encoding="utf-8", errors="ignore"):
            try:
                c = json.loads(ligne).get("compte", "")
            except Exception:
                continue
            if isinstance(c, str) and c:
                comptes.add(c)
    except OSError:
        pass
print("\n".join(sorted(comptes)))
PY
)"

if [ -z "$COMPTES" ]; then
    echo "machines : AUCUNE machine connue — rien à inscrire dans aucune base de compte"
    exit 0
fi

TRAITES=0
for U in $COMPTES; do
    BASE="$DATA/selfs/$U.obrain"
    mkdir -p "$DATA/selfs"
    [ -d "$BASE" ] || "$BIN" init "$BASE" >/dev/null 2>&1
    # ── STDOUT ET STDERR SÉPARÉS, ET C'EST UNE MESURE ───────────────────────
    # Défaut constaté en écrivant ce script : en capturant `2>&1`, le rapport
    # JSON (stdout) se retrouvait collé à la ligne d'état (stderr), ne se
    # parsait plus, et le compte affiché retombait à 0 — pendant que la base
    # contenait bel et bien ses machines (vérifié par requête). Un compte faux
    # vaut moins qu'aucun compte : il fait croire que la passe n'a rien fait
    # alors qu'elle a écrit. On garde donc les flux distincts — le JSON pour
    # compter, l'erreur pour expliquer.
    ERR="$(mktemp)"
    SORTIE="$("$BIN" machines --store "$BASE" --compte "$U" --appliquer 2>"$ERR")"
    CODE=$?
    if [ "$CODE" != 0 ]; then
        # Un échec est DIT, avec son motif : le silence ressemble à une panne.
        echo "machines : $U — ÉCHEC ($CODE) : $(tail -1 "$ERR")" >&2
        rm -f "$ERR"
        continue
    fi
    rm -f "$ERR"
    TRAITES=$((TRAITES + 1))
    if [ "$VERBEUX" = 1 ]; then
        N="$(printf '%s' "$SORTIE" | python3 -c \
             'import sys,json
try: print(json.load(sys.stdin).get("machines",0))
except Exception: print("?")' 2>/dev/null)"
        echo "machines : $U — ${N:-?} machine(s) inscrite(s) dans $BASE"
    fi
done

[ "$TRAITES" = 0 ] && echo "machines : AUCUNE base de compte n'a pu être écrite" >&2
exit 0
