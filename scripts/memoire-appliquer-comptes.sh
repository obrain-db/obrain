#!/usr/bin/env bash
# ── LA PASSE DU SELF DES COMPTES — journal → base dédiée, compte par compte ──
#
# Le chat JOURNALISE (append-only, sans verrou) vers `self:<compte>` quand un
# compte connecté converse ; cette passe APPLIQUE dans `selfs/<compte>.obrain`
# — verrou exclusif sur la PETITE base du compte seulement : le runtime n'est
# jamais touché, l'invariant mono-écrivain vit intact.
#
# (`selfs/<compte>.obrain`, et non `self-<compte>.obrain` : c'est le chemin que
# la conversation RELIT — `engine.rs`, `profil_du_self`, qui prend
# `dirname(store ancré)/selfs/<compte>.obrain`. L'en-tête disait `self-…`, le
# code écrivait `selfs/…` : deux noms pour une seule base est exactement le
# genre d'écart qui fait grandir une base que personne ne relit.)
#
# DÉCOUVERTE PAR LES JOURNAUX, pas par l'annuaire : chaque ligne de journal
# porte son `self` — la passe applique CE QUI ATTEND, sans dépendre du magasin
# IAM (tenu en écriture par le serveur armé, illisible hors bande). Les Self
# de l'agent (`self:agent`, `self:triviere-dsh`) gardent leur circuit à eux.
#
# ── LA GARDE QUI TOMBAIT EN SILENCE (corrigé le 2026-09-16) ─────────────────
# Défaut mesuré : ce script sortait par `[ -d "$J" ] || exit 0` — sans un mot.
# Or le journal est CONSOMMÉ par la passe (`memoire::appliquer` efface le
# fichier une fois les entrées appliquées) : « dossier vide » est donc l'état
# NORMAL après une application réussie, et « dossier disparu » ne l'est pas.
# Les deux se ressemblaient trait pour trait, et le timer
# `obrain-self-appliquer` tournait toutes les 2 minutes en n'écrivant RIEN —
# impossible de distinguer « rien à faire » de « cassé ». Même doctrine que
# l'en-tête de `obrain-continuity.service` : une garde qui tombe en silence ne
# garde rien.
#
# Ce que la passe DIT désormais, et rien de plus :
#   · une ligne quand il n'y a rien à faire — ce qu'elle a regardé et combien
#     de comptes elle a vus (sinon un journal vide ressemble à une panne) ;
#   · une ligne par compte réellement appliqué ;
#   · sur erreur (binaire absent, base verrouillée, application refusée) : la
#     RAISON du moteur, sur stderr, et un code de sortie NON NUL — un refus
#     doit remonter, pas se déguiser en succès.
#
# ── « RIEN À APPLIQUER » N'EST PAS « LA CHAÎNE EST CASSÉE » (2026-09-16) ─────
# Le journal est CONSOMMÉ : un répertoire vide et « 0 compte en attente » sont
# l'état NORMAL après une application réussie. C'est précisément la confusion
# qui a duré jusqu'au 16/09 — ce script disait « 0 en attente » sans pouvoir
# dire s'il regardait une chaîne au repos ou une chaîne morte, et il a fallu
# des centaines de lignes de mesures pour trancher.
# Deux lectures le tranchent désormais, et elles sont ajoutées par cette passe :
#
#   · LE TÉMOIN (`…/memoire-temoins.jsonl`, écrit par `memoire.rs`) — un
#     registre append-only de ce que la mémoire a REÇU et APPLIQUÉ. Il survit à
#     la consommation du journal, donc il répond avec une DATE : « le dernier
#     appel à l'outil de mémoire date de … ». Absent ou sans aucune
#     journalisation, il ne dit pas « repos » : il dit que rien n'a JAMAIS été
#     produit depuis sa mise en place — ce n'est pas la même chose, et
#     maintenant ça ne s'écrit pas pareil ;
#   · LE DISQUE — un témoin qui dit « appliqué dans <base> » alors que `<base>`
#     n'existe plus est une chaîne cassée, pas une chaîne au repos. On le
#     vérifie, et on le dit.
#
# ── LA BASE ÉCRITE EST-ELLE CELLE QU'UNE CONVERSATION RELIRA ? (défaut 5.3) ──
# `engine.rs`, `profil_du_self` : une conversation relit la mémoire du compte
# dans `dirname(store ANCRÉ)/selfs/<compte>.obrain`, et rend `None` SANS LE DIRE
# si ce chemin n'est pas un répertoire. Une conversation ancrée sur un monde
# rangé ailleurs (par exemple `…/mondes/<monde>.obrain`) ne relit donc PAS la
# base que cette passe écrit : la mémoire serait appliquée là où personne ne la
# lit. Cette passe ne peut pas corriger le lecteur (ce n'est pas son fichier),
# mais elle peut REFUSER DE SE TAIRE : la provenance portée par chaque entrée de
# journal contient le store de la conversation, donc on sait calculer où la
# mémoire sera relue — et on le compare à l'endroit où on écrit. Divergence
# ⇒ appliqué quand même (une intention ne se jette pas) mais dit, et la passe
# sort en erreur : un écart qui se répète toutes les deux minutes doit se voir.
set -u

DATA="${OBRAIN_HARNESS_DATA:-$HOME/.local/share/obrain-harness/data}"
BIN="${OBRAIN_BIN:-$HOME/.local/share/obrain-harness/bin/obrain}"
CACHE="${OBRAIN_VIZ_CACHE_DIR:-/tmp/obrain-viz-cache}"
J="$CACHE/memoire"

# ── LE BINAIRE D'ABORD ──────────────────────────────────────────────────────
# Sans lui, la boucle ci-dessous aurait « 0 en attente » pour TOUS les comptes
# (`2>/dev/null` avalait le « command not found ») : la panne la plus grave
# possible prenait la forme exacte du repos. On refuse AVANT de lire quoi que
# ce soit d'autre.
if [ ! -x "$BIN" ]; then
    echo "memoire : binaire INTROUVABLE ($BIN) — rien n'a pu être appliqué" >&2
    exit 3
fi

# ── LE TÉMOIN : LA CHAÎNE A-T-ELLE DÉJÀ PRODUIT, ET QUAND ? ─────────────────
# Lu AVANT tout le reste, et affiché dans TOUS les cas (y compris « aucun
# journal ») : c'est la seule lecture qui date le dernier geste de mémoire. Sans
# elle, « 0 en attente » ne distingue pas une chaîne au repos d'une chaîne morte.
T="$CACHE/memoire-temoins.jsonl"
temoin_brut() {
python3 - "$T" "$DATA" <<'PY'
import sys, os, json, datetime
chemin, data = sys.argv[1], sys.argv[2]
journalises = appliques = 0
dernier_j = dernier_a = None
manques = set()
if os.path.exists(chemin):
    for ligne in open(chemin, encoding="utf-8", errors="ignore"):
        try:
            v = json.loads(ligne)
        except Exception:
            continue
        g, t = v.get("geste"), v.get("t") or 0
        if g == "journalise":
            journalises += 1
            dernier_j = max(dernier_j or 0, t)
        elif g == "applique":
            appliques += 1
            dernier_a = max(dernier_a or 0, t)
            base = v.get("base") or ""
            # LE DISQUE TRANCHE LE RESTE : un témoin qui dit « appliqué dans
            # <base> » alors que <base> n'existe plus est une CHAÎNE CASSÉE, pas
            # une chaîne au repos. On ne juge que les bases de CE répertoire de
            # données : le témoin est partagé, et un autre `OBRAIN_HARNESS_DATA`
            # (une porte d'acceptation, un banc) y écrit ses propres lignes sans
            # que ça concerne cette passe — un faux « cassé » serait aussi
            # trompeur qu'un faux « repos ».
            if base.startswith(data.rstrip("/") + "/") and not os.path.isdir(base):
                manques.add("%s (base %s)" % (v.get("self", "?"), base))

def quand(ms):
    if not ms:
        return "jamais"
    d = datetime.datetime.fromtimestamp(ms / 1000.0)
    return "%s (il y a %.1f h)" % (
        d.strftime("%Y-%m-%dT%H:%M:%S"),
        (datetime.datetime.now() - d).total_seconds() / 3600.0,
    )

if not os.path.exists(chemin):
    print("memoire : témoin %s ABSENT — aucun appel à l'outil de mémoire n'a "
          "jamais été enregistré : ce n'est PAS un repos, c'est une chaîne dont "
          "on n'a aucune trace de production" % chemin)
elif journalises == 0:
    print("memoire : témoin %s — AUCUNE journalisation depuis sa mise en place : "
          "la chaîne n'a jamais rien produit ; ce n'est pas un repos" % chemin)
else:
    print("memoire : témoin %s — %d journalisation(s), dernier appel à l'outil : %s "
          "· %d passe(s) ayant appliqué, dernière : %s"
          % (chemin, journalises, quand(dernier_j), appliques, quand(dernier_a)))
for m in sorted(manques):
    print("#manque:" + m)
PY
}
# ── LA PREMIÈRE LECTURE SERT AU DIAGNOSTIC D'ENTRÉE ─────────────────────────
TEMOIN_BRUT=$(temoin_brut)
TEMOIN_LIGNE=$(printf '%s\n' "$TEMOIN_BRUT" | grep -v '^#manque:' || true)
TEMOIN_MANQUE=$(printf '%s\n' "$TEMOIN_BRUT" | grep '^#manque:' | sed 's/^#manque://' || true)

# ── CE QU'ON A REGARDÉ, MÊME QUAND IL N'Y A RIEN ────────────────────────────
if [ ! -d "$J" ]; then
    echo "memoire : aucun journal ($J absent) — 0 compte ; normal tant qu'un compte n'a pas conversé"
    printf '%s\n' "$TEMOIN_LIGNE"
    exit 0
fi

nb_fichiers=$(find "$J" -maxdepth 1 -name '*.jsonl' -type f 2>/dev/null | wc -l)

# ── QUI ATTEND : les comptes se lisent DANS les journaux ────────────────────
# Le nom du fichier est un HACHAGE du self (`memoire::journal`) : il ne se
# devine pas, il se lit dans la ligne. Un `.jsonl` illisible n'est pas un zéro :
# on le compte et on le dit, sinon un journal corrompu ressemblerait à un
# journal vide.
BRUT=$(python3 - "$J" <<'PY'
import sys, os, json, re
d = sys.argv[1]
comptes = set()
ignores = set()
# Où la conversation RELIRA la mémoire de ce compte : la provenance de chaque
# entrée porte le store ancré (« <monde> · <store> »), donc on sait calculer
# `dirname(store)/selfs/<compte>.obrain` — le chemin EXACT que lit
# `engine.rs::profil_du_self` — et le comparer à celui où cette passe écrit.
# On garde le DERNIER vu : c'est la conversation la plus récente qui dit où la
# mémoire attend d'être relue.
base_relue = {}
for f in sorted(os.listdir(d)):
    if not f.endswith(".jsonl"):
        continue
    try:
        for ligne in open(os.path.join(d, f), encoding="utf-8", errors="ignore"):
            try:
                v = json.loads(ligne)
                s = v.get("self", "")
            except Exception:
                continue
            prov = v.get("provenance") or ""
            store = prov.rsplit(" · ", 1)[-1].strip()
            m = re.fullmatch(r"self:([a-z0-9][a-z0-9_-]{0,63})", s)
            if m:
                if m.group(1) not in ("agent", "triviere-dsh"):
                    comptes.add(m.group(1))
                    if store.startswith("/") and store.endswith(".obrain"):
                        base_relue[m.group(1)] = os.path.join(
                            os.path.dirname(store), "selfs", m.group(1) + ".obrain"
                        )
            elif re.fullmatch(r"self:[^/\\]{1,128}", s):
                # ── UN SELF QUI NE CORRESPOND PAS SE DIT ────────────────────
                # Le motif ci-dessus vient du chat (noms de comptes en
                # minuscules). Un self hors motif — compte à majuscules, nom
                # long — était SAUTÉ SANS UN MOT : sa mémoire restait au
                # journal pour toujours et personne ne le savait. On le
                # rapporte ; le motif continue de protéger contre un `self`
                # fabriqué, mais l'ignorance est DITE.
                ignores.add(s)
    except OSError:
        print(f"#illisible:{f}")
for i in sorted(ignores):
    print(f"#ignore:{i}")
for u, b in sorted(base_relue.items()):
    print(f"#base:{u}\t{b}")
print("\n".join(sorted(comptes)))
PY
)
printf '%s\n' "$BRUT" | grep '^#ignore:' | sed 's/^#ignore:/memoire : self ignoré (hors motif de compte) : /' >&2 || true
printf '%s\n' "$BRUT" | grep '^#illisible:' | sed 's/^#illisible:/memoire : journal illisible : /' >&2 || true
COMPTES=$(printf '%s\n' "$BRUT" | grep -v '^#' || true)

# ── OÙ CHAQUE COMPTE SERA RELU (défaut 5.3) ────────────────────────────────
declare -A BASE_RELUE=()
while IFS=$'\t' read -r u b; do
    [ -n "$u" ] || continue
    BASE_RELUE["$u"]="$b"
done < <(printf '%s\n' "$BRUT" | grep '^#base:' | sed 's/^#base://' || true)

nb_comptes=$(printf '%s\n' "$COMPTES" | grep -c . || true)
traites=0
appliquees=0
echecs=0
# Comptés à part des échecs : une divergence n'empêche RIEN (l'intention est
# appliquée), mais elle signifie que la mémoire atterrit là où aucune
# conversation ne la relira. La confondre avec « aucun problème » serait
# exactement le silence que cette passe corrige.
divergences=0
# Compté APRÈS vérification auprès du binaire : « repéré au journal » (le
# fichier existe) et « en attente » (il y a vraiment des entrées pour ce self)
# ne sont pas la même chose. Les confondre ferait annoncer un compte en attente
# qui n'a rien — « ANNONCER EST DÉCIDER » vaut aussi pour un compte rendu.
comptes_en_attente=0

for U in $COMPTES; do
    [ -n "$U" ] || continue
    traites=$((traites + 1))

    # ── L'ÉTAT DU JOURNAL, AVEC SA RAISON SI ELLE ÉCHOUE ───────────────────
    # `2>/dev/null` rendait un binaire incapable de lire indiscernable d'un
    # journal vide. Et `--format json` est EXPLICITE : le défaut `auto` rend un
    # TABLEAU quand la sortie est un terminal — donc un humain lançant ce script
    # à la main aurait fait échouer le lecteur JSON juste là où il regarde.
    EN_ERR=$(mktemp)
    EN_OUT=$(mktemp)
    # On passe par un FICHIER, pas par un tube : le code de sortie d'un tube est
    # celui de sa DERNIÈRE commande (ici `python3`, toujours 0) — un binaire qui
    # échoue aurait donc rendu « 0 en attente » avec un code 0, c'est-à-dire
    # encore une panne déguisée en repos.
    "$BIN" memoire --self "self:$U" --format json >"$EN_OUT" 2>"$EN_ERR"
    EN_CODE=$?
    if [ -s "$EN_ERR" ]; then
        sed 's/^/memoire : moteur — /' "$EN_ERR" >&2
    fi
    EN=$(python3 -c "
import sys, json
try:
    print(json.load(open(sys.argv[1])).get('en_attente', 0))
except Exception:
    print('')
" "$EN_OUT")
    rm -f "$EN_ERR" "$EN_OUT"
    if [ "$EN_CODE" -ne 0 ] || [ -z "$EN" ]; then
        echo "memoire : self:$U — état du journal ILLISIBLE (code $EN_CODE) : l'entrée n'est pas perdue, elle n'a pas pu être comptée" >&2
        echecs=$((echecs + 1))
        continue
    fi
    [ "${EN:-0}" -gt 0 ] || continue
    comptes_en_attente=$((comptes_en_attente + 1))

    BASE="$DATA/selfs/$U.obrain"
    mkdir -p "$DATA/selfs"

    # ── LA MÉMOIRE VA-T-ELLE ÊTRE RELUE, LÀ OÙ ON L'ÉCRIT ? (défaut 5.3) ────
    # `engine.rs::profil_du_self` relit `dirname(store ancré)/selfs/<compte>.obrain`
    # et rend `None` SANS UN MOT si le chemin n'est pas un répertoire. Une
    # conversation ancrée sur un monde rangé ailleurs écrit donc sa mémoire dans
    # un endroit que personne ne relira — et rien ne le disait. On APPLIQUE
    # quand même (jeter une intention serait pire) mais on le DIT, et on sort en
    # erreur : c'est un écart de configuration qui se répète toutes les deux
    # minutes, pas une condition normale.
    RELUE="${BASE_RELUE[$U]:-}"
    if [ -n "$RELUE" ] && [ "$RELUE" != "$BASE" ]; then
        echo "memoire : self:$U — DIVERGENCE : la conversation s'est ancrée sur un store hors de $DATA, donc elle RELIRA $RELUE, et non $BASE où cette passe écrit. La mémoire serait appliquée là où aucune conversation ne la relit (engine.rs, profil_du_self : dirname(store ancré)/selfs/<compte>.obrain)." >&2
        divergences=$((divergences + 1))
    fi

    if [ ! -d "$BASE" ]; then
        if ! "$BIN" init "$BASE" >/dev/null 2>&1; then
            echo "memoire : self:$U — base $BASE absente et impossible à créer" >&2
            echecs=$((echecs + 1))
            continue
        fi
    fi

    # ── UN AVERTISSEMENT DU MOTEUR N'EST PAS UN REFUS ──────────────────────
    # Défaut constaté en écrivant cette passe : le substrat écrit sur stderr, à
    # chaque ouverture, un « post-replay anchor audit … husk signature »
    # (il le fait même sur une base NEUVE de 0 nœud). La passe réussissait — 1
    # appliquée, nœud présent dans la base — et la version qui jugeait sur « il
    # y a des octets sur stderr » la déclarait REFUSÉE. Le verdict vient donc du
    # code de sortie et du rapport JSON ; stderr est RELAYÉ, jamais interprété.
    OUT_ERR=$(mktemp)
    RAP=$("$BIN" memoire --store "$BASE" --self "self:$U" --appliquer --format json 2>"$OUT_ERR")
    CODE=$?
    # Le verrou se lit AVANT que le fichier temporaire ne soit effacé : la
    # détection sert plus bas, et la lire après le `rm` donnerait toujours faux
    # — un diagnostic qui ne peut pas se déclencher est un diagnostic mort.
    VERROU=0
    if [ -s "$OUT_ERR" ] && grep -q 'already open' "$OUT_ERR"; then
        VERROU=1
    fi
    if [ -s "$OUT_ERR" ]; then
        sed 's/^/memoire : moteur — /' "$OUT_ERR" >&2
        # ── UN AVERTISSEMENT N'EST PAS UN REFUS (défaut 5.4) ───────────────
        # Le substrat écrit à CHAQUE ouverture un « post-replay anchor audit …
        # husk signature », y compris sur une base NEUVE de 0 nœud. Lu dans le
        # journal du timer, il ressemble à l'annonce d'une perte de propriétés —
        # et c'est ainsi qu'on lit une mémoire perdue là où il n'y en a jamais
        # eu. Ce n'est pas un verdict : le verdict est le code de sortie et le
        # rapport JSON. On le dit UNE fois, et on NE FILTRE PAS : filtrer
        # stderr cacherait un vrai refus, qui est justement ce qu'on veut voir.
        echo "memoire : self:$U — $(grep -c . "$OUT_ERR") ligne(s) de moteur relayée(s) : un WARN du substrat (husk signature, émis même sur une base de 0 nœud) n'est PAS un refus ; le verdict est le code de sortie" >&2
    fi
    rm -f "$OUT_ERR"
    if [ "$CODE" -ne 0 ]; then
        # ── UN REFUS PAR VERROU SE NOMME, PARCE QU'IL ARRÊTE LA CROISSANCE ───
        # Mesuré le 2026-09-16 : dès qu'une conversation a relu la base d'un
        # compte (`engine.rs`, `profil_du_self`), le processus du SERVICE garde
        # cette base ouverte — descripteurs relevés dans `/proc/<pid>/fd`, dix
        # fichiers de la base tenus. Un lecteur tient le `flock` PARTAGÉ, donc
        # l'exclusif est refusé et PLUS RIEN ne peut être appliqué pour ce
        # compte tant que le service vit : la base cesse de grandir, et c'est
        # l'objectif même du chantier qui s'arrête. Le message du moteur est
        # relaié tel quel (il nomme le remède : `scripts/memoire.sh`, arrêt,
        # application, relance) ; on ajoute ce qu'il ne dit pas — que l'intention
        # n'est pas perdue, et que la passe réessaiera à chaque tour.
        if [ "$VERROU" = "1" ]; then
            echo "memoire : self:$U — la base est TENUE EN LECTURE (${BASE}) : rien n'est perdu, l'intention reste au journal et cette passe réessaiera toutes les 2 minutes ; mais elle ne peut pas aboutir tant que le détenteur (le service qui a relu le profil de ce compte) tourne." >&2
        fi
        echo "memoire : self:$U — application REFUSÉE (code $CODE) : rien n'est perdu, le journal garde ses entrées" >&2
        echecs=$((echecs + 1))
        continue
    fi

    LIGNE=$(printf '%s' "$RAP" | python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
except Exception:
    print('')
    raise SystemExit
n = d.get('appliquees', 0)
r = d.get('refusees', [])
s = f\"self:$U : {n} appliquee(s)\"
if r:
    s += f\" — {len(r)} REFUSEE(S) : \" + ' | '.join(str(x) for x in r[:3])
print(s)
")
    if [ -z "$LIGNE" ]; then
        echo "memoire : self:$U — rapport d'application illisible : $(printf '%s' "$RAP" | head -c 200)" >&2
        echecs=$((echecs + 1))
        continue
    fi
    echo "memoire : $LIGNE"
    case "$LIGNE" in
        *REFUSEE*) echecs=$((echecs + 1)) ;;
    esac
    n=$(printf '%s' "$LIGNE" | sed -n 's/.*: \([0-9]*\) appliquee.*/\1/p')
    appliquees=$((appliquees + ${n:-0}))
done

if [ "$echecs" -gt 0 ]; then
    echo "memoire : $echecs erreur(s) sur $traites compte(s) traité(s) — rien n'est perdu, les entrées refusées restent au journal" >&2
fi

# ── LE COMPTE RENDU DU REPOS ────────────────────────────────────────────────
# Une seule ligne, et elle dit ce qui a été REGARDÉ : c'est ce qui distingue
# « rien à appliquer » de « la passe ne regarde plus au bon endroit ».
if [ "$appliquees" -eq 0 ]; then
    echo "memoire : journal $J ($nb_fichiers fichier(s)) — $nb_comptes compte(s) repéré(s) au journal, $comptes_en_attente en attente vérifiée, 0 appliquée"
else
    echo "memoire : journal $J — $appliquees entrée(s) appliquée(s) sur $traites compte(s)"
fi

# ── LE TÉMOIN EST RELU APRÈS LA PASSE, PAS AVANT ────────────────────────────
# Défaut vu à l'usage : la ligne était calculée au début et imprimée à la fin,
# donc elle annonçait « 0 passe(s) ayant appliqué · dernière : jamais » juste
# sous la ligne qui venait de dire « 1 entrée(s) appliquée(s) ». Deux lignes
# contradictoires dans le journal d'un timer, c'est exactement le genre de
# bruit qui fait qu'on cesse de le lire. On relit donc le témoin ici.
TEMOIN_BRUT=$(temoin_brut)
TEMOIN_LIGNE=$(printf '%s\n' "$TEMOIN_BRUT" | grep -v '^#manque:' || true)
TEMOIN_MANQUE=$(printf '%s\n' "$TEMOIN_BRUT" | grep '^#manque:' | sed 's/^#manque://' || true)

# ── LA CHAÎNE EST-ELLE AU REPOS, OU MORTE ? ─────────────────────────────────
# Deuxième ligne, TOUJOURS imprimée — y compris quand la passe échoue, parce que
# c'est justement là qu'on veut savoir depuis quand la mémoire n'a rien reçu.
printf '%s\n' "$TEMOIN_LIGNE"

if [ -n "$TEMOIN_MANQUE" ]; then
    while IFS= read -r m; do
        [ -n "$m" ] || continue
        echo "memoire : CHAÎNE CASSÉE — le témoin dit « mémoire appliquée » pour $m, et cette base n'existe plus : ce n'est pas un repos, c'est une mémoire illisible" >&2
    done <<< "$TEMOIN_MANQUE"
    echecs=$((echecs + 1))
fi
if [ "$divergences" -gt 0 ]; then
    echo "memoire : $divergences compte(s) dont la mémoire est écrite là où la conversation ne la relit PAS (voir DIVERGENCE ci-dessus) — l'intention est appliquée, mais elle ne nourrira aucune conversation : le lecteur (engine.rs) et cette passe ne sont pas d'accord sur l'emplacement (défaut 5.3)" >&2
    echecs=$((echecs + 1))
fi

[ "$echecs" -gt 0 ] && exit 1
exit 0
