#!/usr/bin/env bash
# ── PORTE D'ATELIER — GARANTIR L'EXÉCUTION DE CE TYPE DE PROCESSUS ──────────
#
# ## Ce que cette porte garantit, et pourquoi elle existe
#
# Cet atelier fait travailler des AGENTS EN PARALLÈLE dans un dépôt VIVANT, puis
# déploie. C'est un processus qui a produit, en une journée, une série de défauts
# qu'AUCUNE relecture n'avait vus — ils ont tous été trouvés en MESURANT, un par
# un, à la main :
#
#   · onze tests qui mesuraient l'ENVIRONNEMENT au lieu du code (ils passaient ou
#     échouaient selon ce que le shell exportait) ;
#   · des tests qui ÉCRIVAIENT dans le registre VIVANT de la machine — des
#     déclarations de machines de test, pour des stores disparus ;
#   · un binaire servi DIFFÉRENT du binaire sur disque (inode supprimé, service
#     jamais redémarré) ;
#   · une moitié d'annonce perdue AU TRANSPORT, entre deux chantiers qui se
#     partageaient une frontière de fichiers ;
#   · un cache partagé par cinq services, dont AUCUN ne le déclare.
#
# Chacun de ces défauts est INVISIBLE à la relecture et VISIBLE à la mesure. Les
# refaire à la main à chaque vague n'est pas une garantie, c'est de la chance
# répétée. Cette porte les mesure TOUS, dans un ordre fixe, et REFUSE de dire
# « bon » tant qu'un seul manque.
#
# ## La règle de cette porte
#
#   · Elle ne dit jamais « probablement » : chaque garantie affiche ce qu'elle a
#     MESURÉ, puis son verdict.
#   · Ce qu'elle ne peut pas mesurer ÉCHOUE. « Non mesuré » n'est pas « bon ».
#   · Elle FIXE son sujet au lieu de l'hériter : voir « ON NE SE LAISSE PAS
#     IMPOSER SON SUJET » — un défaut qu'elle a trouvé sur elle-même.
#
# ## Les huit garanties
#
#   1. le dépôt est nommable (propre + un tag nomme l'état) — R2 ;
#   2. la suite est honnête dans les DEUX environnements (ambiant ET débarrassé,
#      même nombre de tests : un test qui disparaît n'est pas un test qui passe) ;
#   3. le registre vivant ne porte aucune trace de test ;
#   4. le service sert EXACTEMENT le binaire du disque ;
#   5. aucun service n'est laissé mort ;
#   6. la porte de domaine passe à 100 % ;
#   7. l'artefact par machine est livrable ET porte le binaire courant ;
#   8. l'interface servie est debout.
#   9. le cache partagé par tous les consommateurs s'accorde (une déclaration
#      partielle séparerait des chaînes en silence — c'est le piège documenté).
#  10. le contrat du harness (déléguée à acceptation-harness.sh).
#  11. la portabilité du nœud, mesurée dans des SYSTÈMES RÉELS (déléguée à
#      acceptation-portabilite.sh) — « on doit pouvoir connecter le même partout ».
#
# ## Usage
#
#   scripts/porte-atelier.sh                 # la porte entière : le seul verdict
#   scripts/porte-atelier.sh --familles 3,7  # épreuve partielle (itération)
#
# Une épreuve partielle n'est JAMAIS un verdict : elle l'écrit en toutes lettres,
# et elle refuse de prononcer la phrase de l'atelier. Pour éprouver une garantie,
# on peut aussi PERTURBER ce qu'elle mesure et vérifier qu'elle refuse — par
# exemple déposer un journal de test dans le registre vivant, puis
# `--familles 3` : elle doit refuser.
#
# Elle N'ÉCRIT RIEN dans le dépôt, ne redémarre AUCUN service, et n'écrit dans
# aucune base vivante. Elle lit, elle mesure, elle juge.
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

# ── ON NE SE LAISSE PAS IMPOSER SON SUJET ───────────────────────────────────
#
# Défaut trouvé EN ÉPROUVANT CETTE PORTE : `OBRAIN_BIN` est posé dans
# l'environnement de cette machine, et il désigne une COPIE ANCIENNE
# (~/.local/share/obrain-harness/bin/obrain). La porte le reprenait donc, et
# mesurait un autre moteur que celui du dépôt — puis accusait le domaine pour ce
# que cet autre moteur ne savait pas faire. Une porte qui hérite son sujet du
# shell ne garantit rien : elle mesure ce qu'on lui donne, sans le dire.
#
# Ici le sujet est FIXÉ : les binaires du dépôt. L'héritage est écrasé.
BIN="$REPO/target/release/obrain"
SELFD="$REPO/target/release/selfd"
export OBRAIN_BIN="$BIN"
export SELFD_BIN="$SELFD"

# Le cache est RÉSOLU, pas supposé — même raisonnement. Aujourd'hui aucun service
# ne déclare cette variable (constat à part entière, voir
# docs/FLOTTE-EN-SERVICE-2026-09-16.md), donc le défaut EST l'emplacement vivant.
# Si un service venait à la déclarer, la porte suivrait la déclaration au lieu de
# mesurer à côté d'elle.
CACHE_DECLARE="$(systemctl --user show obrain-viz -p Environment --value 2>/dev/null \
    | tr ' ' '\n' | sed -n 's/^OBRAIN_VIZ_CACHE_DIR=//p' | head -1)"
CACHE="${CACHE_DECLARE:-/tmp/obrain-viz-cache}"
DEF_CACHE="/tmp/obrain-viz-cache"

SERVICES=(obrain-viz obrain-harness-dev this-rs-gateway dsh-web dsh-dev-3083)

echecs=0
verifications=0
mesures=0
ok()   { verifications=$((verifications + 1)); printf '  ✅ %s\n' "$1"; }
ko()   { verifications=$((verifications + 1)); echecs=$((echecs + 1)); printf '  ❌ %s\n' "$1"; }
mes()  { mesures=$((mesures + 1)); printf '  ·  %s\n' "$1"; }
titre(){ printf '\n%s\n' "$1"; }
aide() { sed -n '2,62p' "$0"; }

FAMILLES=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --familles) FAMILLES="${2:-}"; shift 2 ;;
        --aide|-h|--help) aide; exit 0 ;;
        *) printf 'porte-atelier : option inconnue : %s (voir --aide)\n' "$1" >&2; exit 2 ;;
    esac
done
famille() {
    [ -z "$FAMILLES" ] && return 0
    case ",$FAMILLES," in *",$1,"*) return 0 ;; esac
    return 1
}

# ── 1) LE DÉPÔT EST CONNU ───────────────────────────────────────────────────
f1_depot_connu() {
    titre "1) le dépôt est connu : ce que je mesure est nommable"
    local dirty; dirty="$(git status --porcelain | wc -l)"
    if [ "$dirty" = "0" ]; then
        ok "arbre PROPRE ($(git rev-parse --short HEAD))"
    else
        ko "arbre SALE ($dirty fichier(s)) — ce qui est bâti là n'est pas reconstructible (R2)"
        git status --short | head -8 | sed 's/^/      /'
    fi
    local describe; describe="$(git describe --tags 2>/dev/null || true)"
    if [ -n "$describe" ]; then
        ok "un tag nomme cet état : $describe"
    else
        ko "AUCUN tag ne nomme cet état — R2 : rien ne se déploie qui ne soit commité et tagué"
    fi
}

# ── 2) LA SUITE EST HONNÊTE DANS LES DEUX ENVIRONNEMENTS ────────────────────
#
# C'EST LA GARANTIE CENTRALE, et elle vient d'un défaut mesuré : onze tests du
# pont échouaient quand le shell exportait OBRAIN_IAM, et passaient sinon. Un test
# qui dépend de l'environnement de celui qui le lance ne prouve rien — il décore.
# On exécute donc la MÊME suite DEUX FOIS : telle quelle, puis débarrassée des
# variables d'identité. Les deux doivent être vertes, ET compter le MÊME nombre de
# tests — sinon un test a disparu en route, et un test qui disparaît n'est pas un
# test qui passe.
f2_suite_honnete() {
    titre "2) la suite est honnête dans les DEUX environnements"
    mes "exécution AMBIANTE (celle de l'atelier)…"
    local la; la="$(suite_journal ambiant)"
    read -r code_a pass_a fail_a <"$la.resume"
    mes "exécution DÉBARRASSÉE des variables d'identité…"
    local lb; lb="$(suite_journal debarrasse -u OBRAIN_IAM -u OBRAIN_IAM_STORE -u OBRAIN_IAM_TOKEN)"
    read -r code_b pass_b fail_b <"$lb.resume"

    mes "ambiant     : $pass_a passés, $fail_a échoués (exit $code_a)"
    mes "débarrassé  : $pass_b passés, $fail_b échoués (exit $code_b)"

    if [ "$code_a" = "0" ] && [ "$fail_a" = "0" ]; then
        ok "la suite passe dans l'environnement tel qu'il est"
    else
        ko "la suite ÉCHOUE dans l'environnement ambiant ($fail_a échec(s)) — des tests mesurent l'environnement, pas le code"
        grep -E '^test .* FAILED' "$la" | head -6 | sed 's/^/      /'
    fi
    if [ "$code_b" = "0" ] && [ "$fail_b" = "0" ]; then
        ok "la suite passe aussi débarrassée des variables d'identité"
    else
        ko "la suite ÉCHOUE débarrassée ($fail_b échec(s)) — c'est le cas « CI propre »"
        grep -E '^test .* FAILED' "$lb" | head -6 | sed 's/^/      /'
    fi
    if [ "$pass_a" = "$pass_b" ] && [ "$pass_a" != "0" ]; then
        ok "les deux exécutions comptent le MÊME nombre de tests ($pass_a) — aucune ne saute en silence"
    else
        ko "les deux exécutions ne comptent pas pareil (ambiant $pass_a, débarrassé $pass_b) : un test disparaît selon l'environnement"
    fi
    mes "journaux : $la / $lb"
}

suite_journal() { # $1 = étiquette ; le reste = variables à retirer. Rend un chemin.
    # Rien ne doit s'écrire sur la sortie standard ici : la valeur de retour EST
    # le chemin du journal (le message d'avancement se dit DEHORS).
    local etiquette="$1"; shift
    local log; log="$(mktemp)"
    if [ "$#" -gt 0 ]; then
        env "$@" cargo test -p obrain-cli >"$log" 2>&1
    else
        cargo test -p obrain-cli >"$log" 2>&1
    fi
    local code=$?
    local passes fau
    passes="$(grep -oE '[0-9]+ passed' "$log" | awk '{s+=$1} END {print s+0}')"
    fau="$(grep -oE '[0-9]+ failed' "$log" | awk '{s+=$1} END {print s+0}')"
    printf '%s %s %s' "$code" "$passes" "$fau" >"$log.resume"
    echo "$log"
}

# ── 3) LES TESTS NE LAISSENT RIEN DANS LE REGISTRE VIVANT ───────────────────
#
# Défaut mesuré : le cache partagé portait 46 déclarations de moteurs pour des
# stores `/tmp/.tmpXXXX/…` DISPARUS et zéro rattachement réel. Une passe réelle
# aurait donc inscrit dans une VRAIE base des nœuds `machine:` pour des machines
# de test — « annoncer est décider » violé par un effet de bord de test.
#
# On ne peut pas exiger « le cache ne bouge pas » : les services vivants y
# écrivent légitimement (activation, entretien, résonance). On exige donc ce qui
# est VRAI et vérifiable : aucune déclaration de TEST n'y figure.
f3_registre_intact() {
    titre "3) le registre vivant ne porte aucune trace de test"
    if [ ! -d "$CACHE/machines" ]; then
        ok "aucun registre de machines dans $CACHE — rien à polluer"
        return
    fi
    local suspectes
    suspectes="$(grep -lE '"store"[[:space:]]*:[[:space:]]*"/tmp/(\.tmp|obrain-mcp-|acceptation-|preuve-)' \
        "$CACHE"/machines/*.jsonl 2>/dev/null | head -5)"
    if [ -z "$suspectes" ]; then
        ok "aucune déclaration de machine de test dans $CACHE/machines ($(ls "$CACHE"/machines/*.jsonl 2>/dev/null | wc -l) journal(aux))"
    else
        ko "des tests écrivent dans le REGISTRE VIVANT — une passe réelle inscrirait ces machines de test :"
        printf '%s\n' "$suspectes" | sed 's/^/      /'
    fi
}

# ── 4) LE BINAIRE SERVI EST CELUI DU DISQUE ─────────────────────────────────
#
# Défaut mesuré : le service 8432 tenait un inode SUPPRIMÉ (binaire d'une
# construction antérieure) pendant que le disque en portait une autre. Les deux
# se ressemblaient ; seul le hachage les distinguait. Un déploiement qui n'a pas
# redémarré est un déploiement qui n'a pas eu lieu.
f4_binaire_conforme() {
    titre "4) ce que le service tient est ce qu'il y a sur le disque"
    local pid; pid="$(systemctl --user show obrain-viz -p MainPID --value 2>/dev/null || echo 0)"
    if [ -z "$pid" ] || [ "$pid" = "0" ]; then
        ko "le service obrain-viz n'a pas de PID : impossible de mesurer ce qu'il sert"
        return
    fi
    local h_servi h_disque
    h_servi="$(sha256sum "/proc/$pid/exe" 2>/dev/null | cut -d' ' -f1)"
    h_disque="$(sha256sum target/release/obrain 2>/dev/null | cut -d' ' -f1)"
    mes "servi  : ${h_servi:0:16} (PID $pid)"
    mes "disque : ${h_disque:0:16}"
    if [ -n "$h_servi" ] && [ "$h_servi" = "$h_disque" ]; then
        ok "le service sert EXACTEMENT le binaire du disque"
        return
    fi
    ko "le service sert un AUTRE binaire que celui du disque : le déploiement n'a pas été fait, ou le service n'a pas redémarré"
    if readlink "/proc/$pid/exe" 2>/dev/null | grep -q '(deleted)'; then
        mes "l'inode servi est marqué « (deleted) » : une construction a écrasé le fichier sans redémarrage"
    fi
}

# ── 5) AUCUN SERVICE N'EST LAISSÉ MORT ──────────────────────────────────────
#
# On ne garantit pas que les PIDs n'ont pas bougé : une AUTRE session travaille
# dans ce dépôt et redémarre légitimement. On garantit ce qui dépend de NOUS —
# aucun service laissé inactif — et tout mouvement se SIGNALE plutôt que se taire.
f5_services_vivants() {
    titre "5) aucun service n'est laissé mort"
    local s etat
    for s in "${SERVICES[@]}"; do
        etat="$(systemctl --user is-active "$s" 2>/dev/null)"
        if [ "$etat" = "active" ]; then
            ok "$s est active"
        else
            ko "$s est « ${etat:-inconnu} » — un service laissé mort n'est pas un état acceptable"
        fi
    done
}

# ── 6) LA PORTE DE DOMAINE PASSE À 100 % ────────────────────────────────────
#
# Elle est DÉLÉGUÉE, pas réimplémentée : deux juges qui divergeraient seraient
# pires qu'un seul.
f6_domaine() {
    titre "6) la porte d'acceptation de la flotte"
    local logf; logf="$(mktemp)"
    mes "moteur mesuré : $BIN"
    mes "nœud mesuré   : $SELFD"
    if timeout 1800 ./scripts/acceptation-flotte.sh >"$logf" 2>&1; then
        ok "la flotte est nommée, ses capacités dites, mesurées et cloisonnées (100 %)"
        grep -E 'vérifications\)' "$logf" | tail -1 | sed 's/^/      /'
    else
        ko "la porte de la flotte REFUSE — le domaine n'est pas bon :"
        grep -E '❌' "$logf" | grep -v 'vérification' | head -6 | sed 's/^/      /'
    fi
    mes "journal : $logf"
}

# ── 7) L'ARTEFACT PAR MACHINE EST LIVRABLE ET COHÉRENT ──────────────────────
#
# ── CE QUE CETTE FAMILLE A APPRIS SUR ELLE-MÊME (2026-09-16) ────────────────
# Elle comparait l'artefact au binaire `target/release/selfd` — l'édition GLIBC.
# C'était vrai tant que l'empaqueteur ne produisait que celle-là. Depuis qu'il
# PRÉFÈRE le STATIQUE (cible musl), les deux hachages diffèrent légitimement, et
# la porte a REFUSÉ un artefact parfaitement bon. Une porte qui suppose l'édition
# du livrable mesure sa propre hypothèse, pas le livrable.
#
# Elle lit donc ce que le MANIFESTE déclare : quel binaire a été empaqueté, et
# avec quel sha256 — puis elle VÉRIFIE ce binaire-là, et relit l'archive pour
# confirmer que le contenu est bien celui annoncé. C'est la même leçon que la
# famille 9 (« énumérer, ne pas lister ») et que le sujet hérité : suivre la
# réalité au lieu de la supposer.
f7_artefact() {
    titre "7) l'artefact par machine (selfd) est livrable et cohérent"
    local dist="$HOME/.local/share/obrain-selfd/dist"
    if [ ! -d "$dist" ]; then
        ko "aucun artefact empaqueté dans $dist — le binaire par machine n'existe pas comme livrable"
        return
    fi
    local art
    # LE PLUS RÉCENT, pas le premier du glob : l'ordre alphabétique d'un nom
    # d'artefact n'a rien à voir avec l'ordre des constructions, et prendre le
    # premier venu ferait mesurer un artefact périmé — ce qui est exactement le
    # défaut que cette famille dénonce.
    art="$(grep -l "DEPLOYABLE *: *oui" "$dist"/*.manifest.txt 2>/dev/null | xargs -r ls -t 2>/dev/null | head -1)"
    if [ -z "$art" ]; then
        ko "aucun artefact marqué DÉPLOYABLE dans $dist — empaqueter : scripts/selfd-empaqueter.sh"
        return
    fi
    local sha_manifeste binaire edition
    sha_manifeste="$(sed -n 's/^sha256 *: *//p' "$art" | head -1)"
    # `binaire_source` (le chemin) depuis le 2026-09-16 ; à défaut, on le DIT.
    binaire="$(sed -n 's/^binaire_source *: *//p' "$art" | head -1)"
    edition="$(sed -n 's/^edition_liaison *: *//p' "$art" | head -1)"
    mes "artefact : $(basename "$art")"
    mes "binaire empaqueté : ${binaire:-(non déclaré)} ($edition)"

    if [ -z "$binaire" ]; then
        ko "ce manifeste ne nomme PAS le chemin de son binaire (champ binaire_source absent) : on ne peut pas confronter l'artefact à sa source. Réempaqueter avec l'empaqueteur courant."
        return
    fi
    if [ ! -f "$binaire" ]; then
        ko "le manifeste nomme un binaire qui n'existe plus (« $binaire ») : l'artefact n'est plus rattachable à une construction"
        return
    fi
    local h_courant
    h_courant="$(sha256sum "$binaire" | cut -d' ' -f1)"
    if [ "$h_courant" != "$sha_manifeste" ]; then
        ko "l'artefact déployable ne porte PAS la construction COURANTE de son binaire : le manifeste annonce ${sha_manifeste:0:16}, le fichier vaut ${h_courant:0:16} — il a été empaqueté avant la dernière construction"
        return
    fi
    ok "un artefact DÉPLOYABLE porte exactement la construction courante de son binaire (${h_courant:0:16})"

    # Et l'ARCHIVE doit vraiment contenir ce binaire : un manifeste peut mentir,
    # un contenu se relit.
    local archive="${art%.manifest.txt}.tar.gz"
    if [ -f "$archive" ]; then
        local h_dedans
        h_dedans="$(tar -xzOf "$archive" selfd 2>/dev/null | sha256sum | cut -d' ' -f1)"
        if [ "$h_dedans" = "$sha_manifeste" ]; then
            ok "l'archive RELUE porte le binaire annoncé (le manifeste n'est pas une décoration)"
        else
            ko "l'archive ne contient PAS le binaire annoncé (dedans ${h_dedans:0:16}, annoncé ${sha_manifeste:0:16})"
        fi
    else
        ko "l'archive déclarée par le manifeste est absente : $archive"
    fi
}

# ── 8) L'INTERFACE SERVIE N'EST PAS CASSÉE ──────────────────────────────────
#
# `dist` est un lien symbolique vers l'UI servie, et `vite build` VIDE `dist/` au
# début : une reconstruction lancée sans précaution détruit l'interface vivante.
f8_interface() {
    titre "8) l'interface servie n'est pas cassée"
    local lien; lien="$(readlink packages/obrain-dashboard/dist 2>/dev/null || echo '')"
    if [ -n "$lien" ]; then
        ok "packages/obrain-dashboard/dist est toujours un LIEN vers $lien"
    else
        ko "dist n'est plus un lien symbolique — une reconstruction l'a remplacé, l'UI servie est en danger"
    fi
    local chemin code
    for chemin in /app/ /login; do
        code="$(curl -sS -m 8 -o /dev/null -w '%{http_code}' "http://192.168.1.103:8432$chemin" 2>/dev/null || echo 000)"
        if [ "$code" = "200" ]; then
            ok "l'interface répond en 200 sur $chemin"
        else
            ko "l'interface répond $code sur $chemin — elle n'est pas servie"
        fi
    done
}

# ── 9) LE CACHE PARTAGÉ EST UN FAIT, PAS UNE SUPPOSITION ────────────────────
#
# Le constat qui a ouvert cette famille : le cache `OBRAIN_VIZ_CACHE_DIR` est lu
# par NEUF modules (activation, entretien, mémoire, machines, résonance, relais)
# et par CINQ services vivants — et AUCUN d'eux ne déclare la variable. Ils
# partagent donc le défaut `/tmp/obrain-viz-cache` par simple omission.
#
# Tant que personne ne la pose, l'ensemble est cohérent (et fragile : une purge
# de /tmp les perd tous ensemble — défaut connu et documenté). Le danger est
# ailleurs, et il est SILENCIEUX : le jour où UN SEUL service la pose — pour
# rendre son cache persistant, par exemple — sa vue se sépare de celle des autres
# sans que rien ne le dise. La mémoire des comptes cesse alors d'être appliquée,
# ou la flotte cesse d'être connue, et le symptôme n'apparaît que longtemps après
# la cause.
#
# C'est pourquoi cette migration n'a pas été faite à la va-vite : elle se fait
# POUR TOUS OU PAS DU TOUT. Cette famille transforme ce raisonnement en invariant
# mesuré — elle refuse une déclaration PARTIELLE.
f9_cache_partage() {
    titre "9) le cache partagé : TOUS ses consommateurs s'accordent"
    # ── ON ÉNUMÈRE LES UNITÉS, ON NE LES LISTE PAS ──────────────────────────
    # La première version de cette famille parcourait une liste écrite à la main
    # (les cinq services connus). C'était la refaire à l'échelle du problème : une
    # liste manuscrite oublie toujours celle qu'on n'avait pas prévue, et c'est
    # PRÉCISÉMENT l'unité oubliée qui sépare sa vue du cache en silence. On
    # interroge donc TOUTES les unités chargées — et l'on PUBLIE combien on en a
    # examiné, pour que « toutes » soit une mesure et non une affirmation.
    #
    # `systemctl show -p Environment` rend l'environnement FUSIONNÉ : unité,
    # drop-ins, et `EnvironmentFile` s'il existe. C'est la vue réelle du service,
    # pas la lecture d'un fichier.
    local declarees=0 examinees=0 valeurs="" u v
    while read -r u; do
        [ -n "$u" ] || continue
        examinees=$((examinees + 1))
        v="$(systemctl --user show "$u" -p Environment --value 2>/dev/null \
            | tr ' ' '\n' | sed -n 's/^OBRAIN_VIZ_CACHE_DIR=//p' | head -1)"
        if [ -n "$v" ]; then
            declarees=$((declarees + 1))
            valeurs="$valeurs$v
"
            mes "$u déclare : $v"
        fi
    done < <(systemctl --user list-unit-files --type=service --type=timer --no-legend 2>/dev/null | awk '{print $1}')
    mes "$examinees unité(s) examinée(s) — l'énumération est mesurée, pas supposée"

    if [ "$declarees" = "0" ]; then
        ok "aucune des $examinees unités ne déclare OBRAIN_VIZ_CACHE_DIR : elles partagent toutes le défaut"
        mes "défaut partagé : $DEF_CACHE — cohérent, mais une purge de /tmp les perd TOUS ensemble (voir docs/FLOTTE-EN-SERVICE-2026-09-16.md)"
        return
    fi
    # ── LE DÉFAUT EST UNE DÉCLARATION IMPLICITE DE TOUS LES AUTRES ──────────
    # C'est le point que la première version de cette famille ratait : elle
    # comptait les valeurs DÉCLARÉES entre elles sans les comparer au défaut. Une
    # SEULE unité déclarant un chemin différent passait donc pour « cohérente » —
    # alors que c'est exactement la séparation silencieuse qu'on veut interdire :
    # sa vue du cache n'est plus celle de tous ceux qui ne déclarent rien.
    local hors_defaut
    hors_defaut="$(printf '%s' "$valeurs" | grep -v '^$' | grep -vx "$DEF_CACHE" | sort -u)"
    if [ -z "$hors_defaut" ]; then
        ok "$declarees unité(s) la déclarent, toutes AVEC LA VALEUR PAR DÉFAUT — aucune vue séparée"
    else
        ko "SÉPARATION SILENCIEUSE : $declarees unité(s) déclarent OBRAIN_VIZ_CACHE_DIR, et $([ "$(printf '%s\n' "$hors_defaut" | wc -l)" = "1" ] && echo "l'une" || echo "certaines") avec une valeur DIFFÉRENTE du défaut. Les autres continuent sur $DEF_CACHE : ces chaînes ne voient plus le même cache, et le symptôme n'apparaîtra que bien plus tard."
        printf '%s\n' "$hors_defaut" | sed 's/^/      hors défaut : /'
    fi
}

# ── 10) LE CONTRAT DU HARNESS ───────────────────────────────────────────────
#
# Déléguée, comme la porte de domaine : le harness a SA porte
# (`acceptation-harness.sh`, sept familles, 34 vérifications), et fabriquer un
# second juge ici les ferait diverger. C'est celle-là qui a trouvé qu'une session
# INCONNUE rendait « ok » sur un vide — le mode de défaillance que la doctrine
# interdit, par l'autre bout de celui qui avait déjà été corrigé.
f10_harness() {
    titre "10) le contrat du harness (porte dédiée)"
    local logh; logh="$(mktemp)"
    mes "porte : scripts/acceptation-harness.sh --bin $BIN"
    if timeout 1800 ./scripts/acceptation-harness.sh --bin "$BIN" >"$logh" 2>&1; then
        ok "le harness tient son contrat (identité vraie et tracée, injection qui refuse le faux « ok », base servie intacte)"
        grep -E 'vérification' "$logh" | tail -1 | sed 's/^/      /'
    else
        ko "la porte du harness REFUSE :"
        grep -E '❌' "$logh" | grep -v 'vérification' | head -6 | sed 's/^/      /'
    fi
    mes "journal : $logh"
}

# ── 11) LA PORTABILITÉ DU NŒUD ──────────────────────────────────────────────
#
# Déléguée, comme les deux autres portes de domaine. « On doit pouvoir connecter
# le même partout » : le binaire par machine doit démarrer sur la machine du
# client, quelle qu'elle soit. Une garantie qui ne se mesure pas dans des
# SYSTÈMES RÉELS n'est qu'une lecture de manifeste — cette famille exécute
# l'artefact dans chaque image disponible et exige qu'il y démarre, y réponde, et
# y dise qui il est.
f11_portabilite() {
    titre "11) le nœud se connecte PARTOUT (portabilité mesurée en systèmes réels)"
    local logp; logp="$(mktemp)"
    if timeout 1800 ./scripts/acceptation-portabilite.sh >"$logp" 2>&1; then
        ok "le MÊME artefact démarre et répond dans chaque système mesuré, et s'y nomme"
        grep -E 'vérification' "$logp" | tail -1 | sed 's/^/      /'
    else
        ko "la porte de portabilité REFUSE :"
        grep -E '❌' "$logp" | grep -v 'vérification' | head -6 | sed 's/^/      /'
    fi
    mes "journal : $logp"
}

# ── EXÉCUTION ───────────────────────────────────────────────────────────────
printf '════════════════════════════════════════════════════════════════════\n'
if [ -n "$FAMILLES" ]; then
    printf 'PORTE D ATELIER — ÉPREUVE PARTIELLE (familles %s) — %s\n' "$FAMILLES" "$(date -Is)"
    printf 'CE N EST PAS UN VERDICT : les garanties non exécutées ne sont pas mesurées.\n'
else
    printf 'PORTE D ATELIER — %s\n' "$(date -Is)"
fi
printf '════════════════════════════════════════════════════════════════════\n'

famille 1 && f1_depot_connu
famille 2 && f2_suite_honnete
famille 3 && f3_registre_intact
famille 4 && f4_binaire_conforme
famille 5 && f5_services_vivants
famille 6 && f6_domaine
famille 7 && f7_artefact
famille 8 && f8_interface
famille 9 && f9_cache_partage
famille 10 && f10_harness
famille 11 && f11_portabilite

titre "verdict"
printf '  %d vérification(s) · %d mesure(s)\n' "$verifications" "$mesures"
if [ "$echecs" != "0" ]; then
    printf '  ❌ %d GARANTIE(S) NON TENUE(S) sur %d — NE PAS DÉPLOYER, NE PAS CONSIDÉRER COMME BON\n' \
        "$echecs" "$verifications"
    printf '     Chaque ❌ nomme ce qui manque. Une porte qu on assouplit ne garde rien.\n'
    exit 1
fi
if [ -n "$FAMILLES" ]; then
    printf '  ⚠️  ÉPREUVE PARTIELLE (familles %s) : ces garanties-là tiennent, LES AUTRES N ONT\n' "$FAMILLES"
    printf '      PAS ÉTÉ MESURÉES. Pour un verdict, lancer la porte sans --familles.\n'
    exit 0
fi
printf '  ✅ L ATELIER A BIEN EXÉCUTÉ : dépôt nommable, suite honnête dans les deux\n'
printf '     environnements, registre vivant intact, binaire servi conforme, services\n'
printf '     vivants, domaine à 100 %%, artefact livrable, interface debout, cache\n'
printf '     partagé cohérent, contrat du harness tenu, nœud portable partout.\n'
exit 0
