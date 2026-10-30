#!/usr/bin/env bash
# ── PORTE DE PORTABILITÉ — « ON DOIT POUVOIR CONNECTER LE MÊME PARTOUT » ────
#
# ## Le besoin, tel qu'il a été dit
#
# « On doit pouvoir connecter le même partout, et qu'il sache instinctivement qui
# il est où il est. » Deux exigences, et la seconde n'a de sens que si la première
# tient : un nœud qui ne démarre pas sur la machine du client ne saura rien du
# tout.
#
# ## Ce qui bloquait, et comment c'est mesuré
#
# `selfd` exigeait GLIBC 2.39 : il refusait de démarrer sur Debian 11 (2.31),
# Debian 12 (2.36) et Ubuntu 22.04 (2.35). Le binaire STATIQUE (cible musl) n'a
# aucune dépendance à la glibc. Cette porte ne se contente pas de le lire dans un
# manifeste : elle EXÉCUTE l'artefact dans plusieurs systèmes, et exige qu'il y
# démarre, y RÉPONDE, et y dise QUI IL EST.
#
# ## Les quatre garanties
#
#   1. l'artefact est DÉPLOYABLE et déclaré PORTABLE, et le manifeste le prouve
#      (édition de liaison statique, `needed : aucune`) ;
#   2. IL DÉMARRE ET RÉPOND dans CHAQUE image disponible — pas « devrait » ;
#   3. LE CONTRASTE : le binaire dynamique (glibc) ÉCHOUE là où le statique
#      réussit. Sans ce contraste, « portable » serait une affirmation que rien
#      ne distingue d'une chance ;
#   4. il SAIT QUI IL EST OÙ IL EST : le nœud nomme sa machine, et cette machine
#      est CELLE DU CONTENEUR (nom d'hôte imposé) — donc il la LIT, il ne
#      l'invente pas.
#
# ## Ce qu'elle ne fait pas
#
# Elle ne teste que les architectures et les images PRÉSENTES localement : elle
# NOMME celles qu'elle n'a pas, au lieu de les passer sous silence. Une garantie
# qui ne dit pas son périmètre est une garantie qu'on croit plus large qu'elle.
#
# ## Usage
#
#   scripts/acceptation-portabilite.sh                 # le dernier artefact déployable
#   scripts/acceptation-portabilite.sh --artefact <chemin>
#   scripts/acceptation-portabilite.sh --familles 2,4
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

DIST="${SELFD_DIST:-$HOME/.local/share/obrain-selfd/dist}"
# Le binaire DYNAMIQUE, celui qui servira de CONTRASTE : il doit échouer là où le
# statique réussit. C'est la preuve que le statique apporte quelque chose.
DYNAMIQUE="$REPO/target/release/selfd"

# Les systèmes où l'on doit pouvoir se connecter. L'ordre va du plus ancien au
# plus récent, parce que c'est la contrainte qui décide : une glibc ancienne est
# le cas difficile.
VOULUES="debian:11 debian:12 redis:7-alpine"
# Une image musl est indispensable au contraste : sans elle, on ne mesure que des
# glibc plus ou moins récentes.

echecs=0; verifications=0; mesures=0
ok()   { verifications=$((verifications + 1)); printf '  ✅ %s\n' "$1"; }
ko()   { verifications=$((verifications + 1)); echecs=$((echecs + 1)); printf '  ❌ %s\n' "$1"; }
mes()  { mesures=$((mesures + 1)); printf '  ·  %s\n' "$1"; }
titre(){ printf '\n%s\n' "$1"; }

ARTEFACT=""
FAMILLES=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --artefact) ARTEFACT="${2:?--artefact attend un chemin}"; shift 2 ;;
        --familles) FAMILLES="${2:-}"; shift 2 ;;
        --aide|-h|--help) sed -n '2,48p' "$0"; exit 0 ;;
        *) printf 'acceptation-portabilite : option inconnue : %s\n' "$1" >&2; exit 2 ;;
    esac
done
famille() {
    [ -z "$FAMILLES" ] && return 0
    case ",$FAMILLES," in *",$1,"*) return 0 ;; esac
    return 1
}

RACINE="$(mktemp -d /tmp/acceptation-portabilite-XXXXXX)"
nettoyer() { [ "${GARDER_TRACE:-0}" = "1" ] || rm -rf "$RACINE"; }
trap nettoyer EXIT

printf '════════════════════════════════════════════════════════════════════\n'
printf 'PORTE DE PORTABILITÉ — %s\n' "$(date -Is)"
if [ -n "$FAMILLES" ]; then
    printf 'ÉPREUVE PARTIELLE (familles %s) — CE N EST PAS UN VERDICT\n' "$FAMILLES"
fi
printf '════════════════════════════════════════════════════════════════════\n'

# ── DOCKER EST-IL LÀ ? ──────────────────────────────────────────────────────
# Sans conteneurs, cette porte ne peut RIEN mesurer. Elle le dit et refuse :
# « non mesuré » n'est pas « bon ».
if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
    ko "docker est indisponible : la portabilité ne peut pas être MESURÉE ici (et « non mesuré » n'est pas « bon »)"
    printf '\nverdict\n  ❌ 1 GARANTIE(S) NON TENUE(S) : aucune mesure possible\n'
    exit 1
fi

# ── LES IMAGES PRÉSENTES, NOMMÉES ───────────────────────────────────────────
IMAGES=""
ABSENTES=""
for img in $VOULUES; do
    if docker image inspect "$img" >/dev/null 2>&1; then
        IMAGES="$IMAGES $img"
    else
        ABSENTES="$ABSENTES $img"
    fi
done
IMAGES="${IMAGES# }"

# ── 1) L'ARTEFACT DÉCLARE SA PORTABILITÉ ────────────────────────────────────
f1_artefact() {
    titre "1) l'artefact est déployable et DÉCLARÉ portable"
    if [ -z "$ARTEFACT" ]; then
        ARTEFACT="$(grep -l 'DEPLOYABLE *: *oui' "$DIST"/*.manifest.txt 2>/dev/null \
            | xargs -r ls -t 2>/dev/null | head -1)"
        if [ -n "$ARTEFACT" ]; then
            ARTEFACT="${ARTEFACT%.manifest.txt}.tar.gz"
        fi
    fi
    if [ -z "$ARTEFACT" ] || [ ! -f "$ARTEFACT" ]; then
        ko "aucun artefact DÉPLOYABLE trouvé dans $DIST — empaqueter : scripts/selfd-empaqueter.sh"
        return
    fi
    mes "artefact : $(basename "$ARTEFACT")"
    tar -xzf "$ARTEFACT" -C "$RACINE" || { ko "l'archive ne s'extrait pas"; return; }
    [ -x "$RACINE/selfd" ] && ok "l'archive extraite contient un binaire « selfd » exécutable" \
                           || { ko "l'archive ne contient pas de binaire « selfd » exécutable"; return; }
    MAN="$(tar -tzf "$ARTEFACT" | grep MANIFEST | head -1)"
    tar -xzf "$ARTEFACT" -C "$RACINE" "$MAN" 2>/dev/null || true

    local edition portable needed glibc
    edition="$(sed -n 's/^edition_liaison *: *//p' "$RACINE/$MAN" | head -1)"
    portable="$(sed -n 's/^portable *: *//p' "$RACINE/$MAN" | head -1)"
    needed="$(sed -n 's/^needed *: *//p' "$RACINE/$MAN" | head -1)"
    glibc="$(sed -n 's/^glibc_exigee_max *: *//p' "$RACINE/$MAN" | head -1)"
    mes "édition : $edition · glibc exigée : $glibc · needed : $needed"
    case "$edition" in
        *statique*) ok "le manifeste déclare une liaison STATIQUE — le fondement de « le même partout »" ;;
        *) ko "le manifeste ne déclare PAS une liaison statique (édition « $edition ») : cet artefact dépend de la glibc du lieu de construction" ;;
    esac
    if [ "$glibc" = "aucune" ] && [ "$needed" = "aucune" ]; then
        ok "aucune glibc exigée ET aucune bibliothèque partagée déclarée — cohérent avec un statique"
    else
        ko "le manifeste n'est pas cohérent avec un statique (glibc=$glibc, needed=$needed)"
    fi
    case "$portable" in
        oui*) ok "il se DIT portable : ${portable:0:80}…" ;;
        *) ko "il se dit NON portable : $portable" ;;
    esac
    # Le fichier doit vraiment être statique, pas seulement le dire.
    if file -b "$RACINE/selfd" | grep -qE 'static-pie linked|statically linked'; then
        ok "le FICHIER est bien lié statiquement ($(file -b "$RACINE/selfd" | grep -oE 'static[a-z-]* linked'))"
    else
        ko "le fichier n'est PAS statique malgré ce que dit le manifeste : $(file -b "$RACINE/selfd" | head -c 100)"
    fi
}

# ── 2) IL DÉMARRE ET RÉPOND PARTOUT ─────────────────────────────────────────
# Le geste : on impose un NOM D'HÔTE au conteneur, et on exige que le nœud le
# reprenne. C'est la garantie 4 mesurée par la même exécution.
sonde() { # $1 = image ; $2 = binaire hôte ; $3 = nom d'hôte imposé ; rend le statut du nœud
    docker run --rm --network none --hostname "$3" \
        -v "$2:/n/selfd:ro" "$1" \
        /n/selfd --root /tmp --state-dir /tmp/st --socket /tmp/s.sock --call status 2>&1
}

f2_demarre_partout() {
    titre "2) le MÊME artefact démarre et répond dans chaque système"
    if [ -z "$IMAGES" ]; then
        ko "aucune image disponible parmi :$VOULUES — impossible de mesurer"
        return
    fi
    local img sortie
    for img in $IMAGES; do
        sortie="$(sonde "$img" "$RACINE/selfd" "portable-$(echo "$img" | tr ':/' '--')" 2>&1)"
        if printf '%s' "$sortie" | grep -q 'nœud'; then
            ok "$img : le nœud DÉMARRE et répond"
            mes "$img : $(printf '%s' "$sortie" | grep -m1 'nœud')"
        else
            ko "$img : le nœud NE RÉPOND PAS — $(printf '%s' "$sortie" | head -2 | tr '\n' ' ')"
        fi
    done
    [ -n "$ABSENTES" ] && mes "images NON disponibles (périmètre de cette porte, dit sans le cacher) :$ABSENTES"
}

# ── 3) LE CONTRASTE ─────────────────────────────────────────────────────────
# Une garantie sans contraire ne prouve rien : si le binaire glibc réussissait
# partout lui aussi, « portable » ne distinguerait rien. On exige donc qu'il
# ÉCHOUE au moins quelque part — et on NOMME l'endroit, sinon on ne saurait pas
# ce que le statique apporte.
f3_contraste() {
    titre "3) le contraste : le binaire lié à la glibc ÉCHOUE là où le statique réussit"
    if [ ! -x "$DYNAMIQUE" ]; then
        ko "aucun binaire dynamique dans target/release/selfd : le contraste ne peut pas être mesuré"
        return
    fi
    local img sortie echecs_attendus=0 noms=""
    for img in $IMAGES; do
        sortie="$(sonde "$img" "$DYNAMIQUE" "contraste-$(echo "$img" | tr ':/' '--')" 2>&1)"
        if printf '%s' "$sortie" | grep -q 'nœud'; then
            mes "$img : le binaire glibc démarre AUSSI (cette image est assez récente pour lui)"
        else
            echecs_attendus=$((echecs_attendus + 1))
            noms="$noms $img"
            mes "$img : le binaire glibc est REFUSÉ — $(printf '%s' "$sortie" | grep -m1 -oE 'GLIBC_[0-9.]+|not found' | head -1)"
        fi
    done
    if [ "$echecs_attendus" -gt 0 ]; then
        ok "le contraste est MESURÉ : le binaire dynamique échoue sur$noms, le statique y réussit"
    else
        ko "AUCUN contraste : le binaire dynamique démarre partout où le statique démarre. Sur ce parc d'images, « portable » n'est PAS distingué — élargir le parc (Debian 11, Alpine) avant de conclure"
    fi
}

# ── 4) IL SAIT QUI IL EST OÙ IL EST ─────────────────────────────────────────
# « instinctivement » = sans configuration. On ne passe NI --machine NI aucune
# variable : le nœud doit lire son nom d'hôte du NOYAU et le rendre. Si l'on
# imposait le nom, on mesurerait notre propre argument.
f4_identite() {
    titre "4) il sait QUI IL EST et OÙ IL EST — sans qu'on le lui dise"
    local img sortie hote machine racine
    for img in $IMAGES; do
        hote="portable-$(echo "$img" | tr ':/' '--')"
        sortie="$(sonde "$img" "$RACINE/selfd" "$hote" 2>&1)"
        if ! printf '%s' "$sortie" | grep -q 'nœud'; then
            ko "$img : aucune identité rendue (le nœud n'a pas répondu)"
            continue
        fi
        machine="$(printf '%s' "$sortie" | sed -n 's/^machine *: *//p' | head -1)"
        racine="$(printf '%s' "$sortie" | sed -n 's/^racine *: *//p' | head -1)"
        if [ "$machine" = "$hote" ]; then
            ok "$img : il se nomme « $machine » — le nom d'hôte du conteneur, qu'il a LU (rien ne lui a été passé)"
        else
            ko "$img : il se nomme « $machine » alors que la machine s'appelle « $hote » : il n'a pas lu son nom"
        fi
        if [ -n "$racine" ] && [ "$racine" != "(aucune)" ]; then
            ok "$img : il dit OÙ il tourne (racine « $racine »)"
        else
            ko "$img : il ne dit pas où il tourne (racine « $racine »)"
        fi
        # ── SA PLACE DANS LA FLOTTE : LE SOCLE ET SON ÉTAT ────────────────────
        # « où il est » ne s'arrête pas à la racine : le nœud doit dire QUEL
        # MOTEUR il sert, s'il le joint, et où il en est sur ce socle. Dans un
        # conteneur `--network none` le moteur est absent — et c'est justement le
        # cas intéressant : un nœud qui se TAIRAIT ne saurait pas où il est, un
        # nœud qui DIT « injoignable » et « inconnue » sait exactement où il en
        # est. On exige donc qu'il PARLE, pas qu'il réussisse.
        if printf '%s' "$sortie" | grep -q '^moteur'; then
            ok "$img : il NOMME le moteur qu'il sert — $(printf '%s' "$sortie" | grep -m1 '^moteur' | cut -c1-64)"
        else
            ko "$img : il ne dit pas quel moteur il sert : il agit sans dire à quel socle il appartient"
        fi
        if printf '%s' "$sortie" | grep -qE 'joignable|INJOIGNABLE'; then
            ok "$img : il dit si son socle est JOIGNABLE (mesuré, pas supposé)"
        else
            ko "$img : il ne dit pas si son socle est joignable"
        fi
        if printf '%s' "$sortie" | grep -q '^place'; then
            ok "$img : il dit sa PLACE — y compris « inconnue », quand il ne peut pas la connaître"
        else
            ko "$img : il tait sa place : le silence se lirait comme « pas rattaché »"
        fi
    done
    # Et le nœud doit être DISTINCT d'un système à l'autre : deux conteneurs ne
    # doivent pas produire le même identifiant, sinon « qui je suis » ne
    # distingue rien.
    local n1 n2
    n1="$(sonde "$(echo "$IMAGES" | awk '{print $1}')" "$RACINE/selfd" "ident-a" 2>&1 | sed -n 's/^machine *: *//p' | head -1)"
    n2="$(sonde "$(echo "$IMAGES" | awk '{print $1}')" "$RACINE/selfd" "ident-b" 2>&1 | sed -n 's/^machine *: *//p' | head -1)"
    if [ -n "$n1" ] && [ -n "$n2" ] && [ "$n1" != "$n2" ]; then
        ok "deux machines différentes se disent DIFFÉRENTES ($n1 ≠ $n2)"
    else
        ko "deux machines différentes se disent IDENTIQUES ($n1 = $n2) : l'identité ne distingue pas les lieux"
    fi
}

famille 1 && f1_artefact
famille 2 && f2_demarre_partout
famille 3 && f3_contraste
famille 4 && f4_identite

titre "verdict"
printf '  %d vérification(s) · %d mesure(s)\n' "$verifications" "$mesures"
if [ "$echecs" != "0" ]; then
    printf '  ❌ %d GARANTIE(S) NON TENUE(S) sur %d — « le même partout » n est pas établi\n' "$echecs" "$verifications"
    [ "${GARDER_TRACE:-0}" = "1" ] && printf '  trace : %s\n' "$RACINE"
    exit 1
fi
if [ -n "$FAMILLES" ]; then
    printf '  ⚠️  ÉPREUVE PARTIELLE (familles %s) : LES AUTRES GARANTIES N ONT PAS ÉTÉ MESURÉES.\n' "$FAMILLES"
    exit 0
fi
printf '  ✅ LE MÊME BINAIRE SE CONNECTE PARTOUT (au parc mesuré) : il démarre et répond\n'
printf '     dans chaque système, le binaire lié à la glibc y échoue là où lui réussit,\n'
printf '     et il dit QUI IL EST OÙ IL EST en lisant son nom d hôte — sans configuration.\n'
exit 0
