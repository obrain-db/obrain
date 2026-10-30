#!/usr/bin/env bash
# ── LA FLOTTE ENTRE DANS LE GRAPHE — la passe, et ce qu'elle refuse ─────────
#
# `obrain machines --appliquer` fait de la flotte un FAIT DU GRAPHE : un nœud
# `machine:<hôte>` par machine connue, portant son compte, ses capacités
# annoncées, sa première attache et sa dernière présence (`machines.rs`). Ce
# script est le DÉCLENCHEUR de cette passe : c'est lui que le timer
# `obrain-machines` appelle, et c'est lui qu'un humain lance à la main pour
# vérifier. Une passe qu'aucun geste ne déclenche n'existe pas pour
# l'utilisateur — elle n'a été prouvée que par la porte d'acceptation, qui la
# lance elle-même.
#
# ── POURQUOI CETTE PASSE EST UN GESTE SÉPARÉ ────────────────────────────────
# Le moteur sert ses bases en LECTURE (flock partagé) : il ne PEUT pas écrire
# ces nœuds lui-même, et ce n'est pas un défaut à contourner — c'est
# l'invariant mono-écrivain qui protège les données. Le motif est celui de
# `memoire.rs`, appliqué à la flotte :
#
#   1. le registre JOURNALISE ce qu'il apprend (append-only, aucun verrou) ;
#   2. cette passe APPLIQUE (elle prend l'EXCLUSIF, écrit en `MERGE` idempotent).
#
# ── LA BASE VISÉE, ET LE REFUS QU'ON NE DOIT PAS TAIRE ──────────────────────
# Le défaut est la base que le harness SERT (`runtime-dev.obrain`) : c'est là
# que la question « quelles machines connais-tu ? » se pose au graphe. Mais
# cette base est tenue en LECTURE par les services qui la servent (`viz serve`
# prend un flock partagé, le harness dev un exclusif) : l'ouverture exclusive
# de la passe est alors REFUSÉE. Le refus est DIT, avec son remède, et il
# remonte en code de sortie non nul — ce script n'arrête ni ne redémarre jamais
# un service (R6), et il ne boucle pas non plus : un timer qui réessaie en
# silence pendant qu'un humain croit que la flotte entre dans le graphe est
# exactement le mensonge que ce dépôt refuse.
#
#   Remède 1 — viser une base que NUL service ne tient :
#              scripts/machines-appliquer-comptes.sh (selfs/<compte>.obrain) ;
#   Remède 2 — viser la base d'un compte précis :
#              scripts/machines-appliquer.sh --compte <c> \
#                  --store ~/.local/share/obrain-harness/data/selfs/<c>.obrain
#   Remède 3 — arrêter le service SOI-MÊME le temps d'une passe (jamais ici).
#
# ── L'AUTRE PIÈGE, CELUI QUI NE SE VOIT PAS : REGARDER AU MAUVAIS ENDROIT ───
# Le registre vit sous `OBRAIN_VIZ_CACHE_DIR` (défaut `/tmp/obrain-viz-cache`),
# et c'est le service `obrain-viz` qui l'ÉCRIT. Si l'écrivain et cette passe ne
# pointent pas le même dossier, la passe ne trouve aucun journal et annonce
# tranquillement « 0 machine » : une panne de configuration prend alors la forme
# exacte du repos. Ce script compare donc les deux emplacements connus (le
# défaut /tmp et le chemin persistant que pose le drop-in
# `obrain-viz.service.d-cache-persistant.conf`) et REFUSE de conclure quand le
# registre est ailleurs qu'où il regarde.
#
# Usage :
#   scripts/machines-appliquer.sh [--store <base>] [--compte <c>] [--verbeux]
#   OBRAIN_MACHINES_STORE  base cible (défaut : la base servie par le harness)
#   OBRAIN_HARNESS_DATA    où vivent les bases (défaut ~/.local/share/obrain-harness/data)
#   OBRAIN_VIZ_CACHE_DIR   où vit le registre des machines
#   OBRAIN_BIN             binaire à utiliser (défaut : le déploiement, puis le dépôt)
#
# Codes de sortie : 0 = appliqué, ou rien à appliquer (et il le DIT) ;
#                   1 = passe refusée (verrou tenu, refus moteur, rapport muet) ;
#                   2 = binaire introuvable ; 3 = binaire sans la sous-commande ;
#                   4 = registre introuvable là où l'écrivain l'écrit.
set -u

VERBEUX=0
STORE="${OBRAIN_MACHINES_STORE:-}"
COMPTE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --store)   STORE="${2:-}"; shift 2 ;;
        --compte)  COMPTE="${2:-}"; shift 2 ;;
        --verbeux) VERBEUX=1; shift ;;
        -h|--help)
            sed -n '2,60p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0 ;;
        *)
            echo "machines : argument inconnu « $1 » (voir --help)" >&2
            exit 2 ;;
    esac
done

DATA="${OBRAIN_HARNESS_DATA:-$HOME/.local/share/obrain-harness/data}"
# ── LE DÉFAUT EST LA BASE SERVIE, ET IL SE DIT ──────────────────────────────
# `runtime-dev.obrain` : c'est la base que le harness (8090) sert et que le
# dashboard expose. C'est là que la question se pose ; c'est donc là que la
# réponse doit être écrite — quand la base est libre.
[ -n "$STORE" ] || STORE="$DATA/runtime-dev.obrain"

# ── LE BINAIRE D'ABORD ──────────────────────────────────────────────────────
# Sans lui, la boucle ci-dessous aurait « 0 machine » pour tout le monde : la
# panne la plus grave possible prendrait la forme exacte du repos.
BIN="${OBRAIN_BIN:-}"
if [ -z "$BIN" ]; then
    for c in "$HOME/.local/share/obrain-harness/bin/obrain" \
             "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/target/release/obrain"; do
        [ -x "$c" ] && { BIN="$c"; break; }
    done
fi
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
    echo "machines : binaire INTROUVABLE — posez OBRAIN_BIN=<chemin> : rien n'a pu être appliqué" >&2
    exit 2
fi

# ── LA GARDE QUI PARLE ──────────────────────────────────────────────────────
# Un binaire déployé AVANT l'écriture de la passe ne connaît pas la
# sous-commande : chaque passage échouerait, et personne ne lit les journaux
# d'un timer. On le dit UNE fois, fort, avec le geste qui répare.
if ! "$BIN" machines --help >/dev/null 2>&1; then
    echo "machines : ce binaire ne connaît pas la sous-commande « machines » —" >&2
    echo "           la flotte n'entrera PAS dans le graphe." >&2
    echo "           binaire : $BIN" >&2
    echo "           Reconstruire puis déployer : cargo build --release -p obrain-cli" >&2
    exit 3
fi

# ── LA CIBLE EXISTE-T-ELLE ? SINON ON LA CRÉERAIT SANS LE DIRE ──────────────
# `open_substrate` CRÉE un store quand `substrate.meta` est absent
# (`obrain-engine/src/database/mod.rs`, `open_substrate_inner`). Un chemin
# erroné — faute de frappe, variable vide — ferait donc naître une base neuve,
# y écrirait les nœuds `machine:` et rendrait un rapport de SUCCÈS : la flotte
# serait « dans le graphe » d'une base que personne ne lit. On refuse, et on
# nomme le geste qui crée une base quand c'est bien ce qu'on veut.
if [ ! -f "$STORE/substrate.meta" ]; then
    echo "machines : la base cible n'existe pas ($STORE/substrate.meta absent) — rien n'a été écrit" >&2
    echo "           la passe ne la crée pas : une flotte écrite dans une base neuve que" >&2
    echo "           personne ne sert aurait la forme d'un succès." >&2
    echo "           Créer la base si c'est bien la cible voulue : $BIN init $STORE" >&2
    exit 5
fi

# ── OÙ VIT LE REGISTRE, ET OÙ IL DEVRAIT VIVRE ──────────────────────────────
# `PERSISTANT` est le chemin que pose le drop-in livré dans le dépôt
# (`deploy/systemd/obrain-viz.service.d-cache-persistant.conf`) : la passe doit
# le connaître pour pouvoir DIRE qu'elle ne regarde pas au même endroit que le
# service qui écrit.
CACHE="${OBRAIN_VIZ_CACHE_DIR:-/tmp/obrain-viz-cache}"
J="$CACHE/machines"
PERSISTANT="${OBRAIN_MACHINES_CACHE_PERSISTANT:-$HOME/.local/share/obrain-harness/viz-cache}"
JP="$PERSISTANT/machines"

if [ ! -d "$J" ]; then
    # Le registre peut être AILLEURS : soit à l'ancien emplacement (l'écrivain
    # est resté sur /tmp, la passe a été déplacée), soit au nouveau (le drop-in
    # est posé, la passe ne l'est pas). Dans les deux cas, conclure « 0 machine »
    # serait un mensonge par omission.
    AUTRE=""
    [ "$CACHE" != "$PERSISTANT" ] && [ -d "$JP" ] && AUTRE="$JP"
    [ "$CACHE" != "/tmp/obrain-viz-cache" ] && [ -d "/tmp/obrain-viz-cache/machines" ] \
        && AUTRE="${AUTRE:-/tmp/obrain-viz-cache/machines}"
    if [ -n "$AUTRE" ]; then
        echo "machines : registre INTROUVABLE en $J, mais un registre existe en $AUTRE" >&2
        echo "           l'écrivain (obrain-viz) et cette passe ne pointent pas le même dossier :" >&2
        echo "           appliquer ici écrirait 0 machine en silence." >&2
        echo "           Leur donner le même OBRAIN_VIZ_CACHE_DIR (voir" >&2
        echo "           docs/FLOTTE-EN-SERVICE-2026-09-16.md)." >&2
        exit 4
    fi
    echo "machines : aucun registre ($J absent) — 0 machine connue ; normal tant qu'aucune machine ne s'est rattachée"
    exit 0
fi

# ── CE QU'ON A REGARDÉ, MÊME QUAND IL N'Y A RIEN ────────────────────────────
# Le nom du fichier de journal est un HACHAGE du compte : il ne se devine pas,
# la machine se lit DANS la ligne (`remote.rs`, `ligne_attache`). Un `.jsonl`
# illisible n'est pas un zéro : on le compte et on le dit.
#
# ── DEUX GENRES DANS LE MÊME JOURNAL, ET ON COMPTE LES DEUX ────────────────
# `"kind":"attache"` : une machine sur laquelle un `selfd` s'est attaché (le
# registre). `"kind":"machine"` : un MOTEUR qui déclare sa propre identité au
# démarrage (`harness_serve.rs`, section « l'identité de la machine ») — une
# machine sans `selfd` resterait sinon innommable. La passe écrit les deux ; ne
# compter que le premier genre rendrait ce script AVEUGLE au second : il
# annoncerait « 0 machine connue, rien à appliquer » (sortie 0) pendant que la
# passe aurait des nœuds à écrire. On compte donc toute ligne qui porte un nom de
# machine, quel que soit son genre — même tolérance que le lecteur, et elle ne
# peut pas inventer une machine : un nom vide ou absent n'est pas compté.
BRUT=$(python3 - "$J" <<'PY'
import sys, os, json
d = sys.argv[1]
machines, illisibles = set(), []
for f in sorted(os.listdir(d)):
    if not f.endswith(".jsonl"):
        continue
    try:
        for ligne in open(os.path.join(d, f), encoding="utf-8", errors="ignore"):
            try:
                l = json.loads(ligne)
            except Exception:
                continue
            if not isinstance(l, dict):
                continue
            m = l.get("machine", "")
            if isinstance(m, str) and m:
                machines.add(m)
    except OSError:
        illisibles.append(f)
for f in illisibles:
    print(f"#illisible:{f}")
for m in sorted(machines):
    print(f"#machine:{m}")
PY
)
printf '%s\n' "$BRUT" | grep '^#illisible:' \
    | sed 's/^#illisible:/machines : journal illisible (compté pour rien) : /' >&2 || true
NBF=$(find "$J" -maxdepth 1 -name '*.jsonl' -type f 2>/dev/null | wc -l)
NBM=$(printf '%s\n' "$BRUT" | grep -c '^#machine:' || true)
LISTE=$(printf '%s\n' "$BRUT" | sed -n 's/^#machine://p' | paste -sd, -)

# ── RIEN À APPLIQUER N'EST PAS UNE ERREUR, ET SE DIT ────────────────────────
if [ "$NBM" -eq 0 ]; then
    echo "machines : registre $J ($NBF fichier(s)) — 0 machine connue, rien à appliquer"
    exit 0
fi

# ── LA PASSE ────────────────────────────────────────────────────────────────
# ── STDOUT ET STDERR SÉPARÉS, ET C'EST LA MÊME LEÇON QUE LA PASSE DU SELF ───
# En capturant `2>&1`, le rapport JSON se retrouve collé à la ligne d'état,
# ne se parse plus, et le compte retombe à 0 — pendant que la base contient bel
# et bien ses machines. Un compte faux vaut moins qu'aucun compte.
#
# Et pas par un TUBE : le code de sortie d'un tube est celui de sa DERNIÈRE
# commande, donc « je n'ai pas pu ouvrir la base » serait sorti avec le code de
# `python3`, c'est-à-dire 0 — encore une panne déguisée en repos.
ERR=$(mktemp); OUT=$(mktemp)
if [ -n "$COMPTE" ]; then
    "$BIN" machines --store "$STORE" --compte "$COMPTE" --appliquer --format json \
        >"$OUT" 2>"$ERR"
else
    "$BIN" machines --store "$STORE" --appliquer --format json >"$OUT" 2>"$ERR"
fi
CODE=$?
RAISON=$(tail -1 "$ERR" 2>/dev/null || true)
[ -s "$ERR" ] && sed 's/^/machines : moteur — /' "$ERR" >&2
rm -f "$ERR"

if [ "$CODE" -ne 0 ]; then
    echo "machines : $STORE — la passe a ÉCHOUÉ (code $CODE) ; le registre ($NBM machine(s)) est intact" >&2
    # ── LE REFUS QUI A UN REMÈDE SE DIT AVEC SON REMÈDE ────────────────────
    # Mesuré : l'ouverture est un `flock` EXCLUSIF non bloquant
    # (`obrain-substrate/src/file.rs`, `try_lock_exclusive`) — la passe ne
    # patiente donc pas, elle refuse. Le message du moteur nomme le verrou ;
    # c'est le cas NORMAL d'une base servie, et il ne doit pas se confondre
    # avec une panne.
    case "$RAISON" in
        *"already open"*|*"lock"*|*"flock"*|*"WouldBlock"*|*"read-only"*)
            echo "           LA BASE EST TENUE EN LECTURE par un service (dashboard/harness)." >&2
            echo "           Ce n'est PAS une panne : c'est l'invariant mono-écrivain." >&2
            echo "           Remèdes, dans l'ordre du moins intrusif :" >&2
            echo "             1. viser une base que nul service ne tient :" >&2
            echo "                scripts/machines-appliquer-comptes.sh  (selfs/<compte>.obrain)" >&2
            echo "             2. viser la base d'un compte précis :" >&2
            echo "                $0 --compte <compte> --store $DATA/selfs/<compte>.obrain" >&2
            echo "             3. arrêter VOUS-MÊME le service le temps d'une passe (R6 : ce" >&2
            echo "                script ne le fait jamais)." >&2
            ;;
    esac
    exit 1
fi

RAP=$(cat "$OUT"); rm -f "$OUT"
LIGNE=$(printf '%s' "$RAP" | python3 -c '
import sys, json
try:
    d = json.load(sys.stdin)
except Exception:
    print("")
    raise SystemExit
print("{}|{}|{}".format(d.get("machines", "?"), d.get("connues", "?"),
                       len(d.get("refusees", []))))
' 2>/dev/null)
if [ -z "$LIGNE" ]; then
    echo "machines : rapport d'application ILLISIBLE : $(printf '%s' "$RAP" | head -c 200)" >&2
    exit 1
fi
ECRITES=$(printf '%s' "$LIGNE" | cut -d'|' -f1)
CONNUES=$(printf '%s' "$LIGNE" | cut -d'|' -f2)
REFUSEES=$(printf '%s' "$LIGNE" | cut -d'|' -f3)

# ── LE REGISTRE PORTE DES NOMS QUE LA PASSE N'A PAS RECONNUS ───────────────
# Mesuré sur le journal tel qu'il est écrit : `"kind":"attache"` (le registre,
# qui exige un `id` et un `compte`) et `"kind":"machine"` (la déclaration d'un
# moteur, qui n'en a pas). Un binaire ANTÉRIEUR aux déclarations ignore les
# secondes ; une ligne incomplète est ignorée au rejeu, quel que soit le genre.
# Dans les deux cas la passe connaît 0 machine alors que le journal en nomme :
# écrire « 0 nœud écrit sur 0 connue » aurait la forme exacte d'un repos, et
# c'est faux. On nomme le fait, les deux causes possibles, et on échoue.
if [ "$CONNUES" = "0" ] && [ "$NBM" -gt 0 ]; then
    echo "machines : $STORE — le registre nomme $NBM machine(s) ($LISTE) mais la passe n'en connaît AUCUNE :" >&2
    echo "           la flotte N'EST PAS dans le graphe. Deux causes possibles :" >&2
    echo "             · ce binaire est antérieur aux déclarations de moteurs (kind \"machine\") :" >&2
    echo "               reconstruire et déployer — cargo build --release -p obrain-cli ;" >&2
    echo "             · des lignes sont incomplètes (un rattachement sans « id » ni « compte »" >&2
    echo "               est ignoré au rejeu : remote.rs, depuis_ligne)." >&2
    exit 1
fi

# ── LE DÉFAUT EXACT QUE CETTE GARDE EXISTE POUR ATTRAPER ────────────────────
# Le registre portait des machines ET la passe a rendu 0 nœud écrit : c'est
# l'écart qui ressemblait à « rien à faire ». Il se dit et il échoue.
if [ "$ECRITES" = "0" ] && [ "$CONNUES" != "0" ]; then
    echo "machines : $STORE — 0 nœud écrit alors que le registre porte $CONNUES machine(s) :" >&2
    echo "           la flotte N'EST PAS dans le graphe. Rapport : $(printf '%s' "$RAP" | tr -d '\n' | head -c 200)" >&2
    exit 1
fi
if [ "$REFUSEES" != "0" ]; then
    echo "machines : $STORE — $REFUSEES nœud(s) REFUSÉ(S) par le moteur : $(printf '%s' "$RAP" | tr -d '\n' | head -c 300)" >&2
    exit 1
fi

# ── LE COMPTE RENDU, ET RIEN DE PLUS ───────────────────────────────────────
# Une ligne en régime normal : elle dit la base, ce qui a été écrit, et ce qui
# a été REGARDÉ (le registre et son nombre de fichiers) — c'est ce qui distingue
# « rien à appliquer » de « la passe ne regarde plus au bon endroit ».
echo "machines : $STORE — $ECRITES nœud(s) « machine: » écrit(s) sur $CONNUES connue(s) · registre $J ($NBF fichier(s))"
if [ "$VERBEUX" = 1 ]; then
    echo "machines : connues — $LISTE"
fi
exit 0
