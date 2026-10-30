#!/usr/bin/env bash
# Lance/relance le Dashboard obrain (détaché, log /tmp/vizserve.log),
# ── CE SCRIPT NE CONSTRUIT RIEN, ET DEPUIS LE 2026-09-15 CE N'EST PLUS UN PIEGE ──
# Il lance `./target/release/obrain` tel quel : c'est toujours à vous de
# construire. Mais l'avertissement qui vivait ici est PERIME, et le laisser
# coûterait exactement ce qu'il prétendait éviter — un opérateur qui croit
# devoir passer des options, ou qui doute d'un binaire sain.
#
# CE QU'IL DISAIT : qu'un `cargo build --release -p obrain-cli` « ordinaire »
# dégradait le déploiement, faute de `vector-index`, `text-index`,
# `hybrid-search`, puis d'`iam` — constaté trois fois, la garde redevenant
# inerte sans que rien d'autre ne change. C'etait vrai, et c'etait un défaut de
# CONCEPTION : la completude d'un deploiement dependait d'une option tapee a la
# main, donc d'un geste qu'on oublie.
#
# CE QUI EST VRAI MAINTENANT (`crates/obrain-cli/Cargo.toml`, politique
# « meilleur par defaut » renforcee le 2026-09-15) :
#     default = ["gql", "cypher", "indexer", "ai", "iam", "algos", "parallel"]
# `ai` tire `vector-index`, `text-index`, `hybrid-search`, `cdc`. Un build par
# defaut ne PEUT plus produire un binaire ampute d'une capacite servie : il n'y
# a donc plus de commande speciale a retenir, et `--features ai,iam` reste
# valide mais devient redondant.
#
# LE CONTROLE, LUI, RESTE NECESSAIRE — c'est le seul qui parle du binaire reel :
#     bash scripts/verif-chaine-iam.sh     # sort en 2 si l'IAM n'est pas compile
# Verifie sur le binaire de ce worktree le 2026-09-15 : « CHAINE COMPLETE — tout
# passe » (accueil → /login, refus, connexion, cookie, rotation de jeton,
# revocation, deconnexion).
#
# puis PRÉCHAUFFE les caches disque d'agrégats de chaque store (modules,
# overview, arcs pour les gros stores de code) — ainsi la première visite
# utilisateur est instantanée, même juste après un déploiement/redémarrage.
set -e
HOST=192.168.1.103
PORT=8432
DEFAULT="Runtime dev (engrams)"
DATA=/home/triviere/.local/share/obrain-harness/data
# Base synaptique durable de l'agent (vue Cosmos — la toile de l'univers).
# Retissée par `scripts/weave.sh` (obrain viz weave).
SYNAPSE="$DATA/agent-synapse-triviere-dsh.obrain"
PRESETS=(
  "Runtime dev (engrams)=$DATA/runtime-dev.obrain"
  "Runtime prod=$DATA/runtime-prod.obrain"
  "Self-demo (engrams)=$DATA/self-demo.obrain"
  "Linux complet (v6.6)=/tmp/linux-full3.obrain"
  "Linux e1000=/tmp/kv-linux-e1000.obrain"
  "Obrain (rust)=/tmp/engine_idx.obrain"
  # Store où la courbure de Ricci est RÉELLE et de signes MÊLÉS (mesuré :
  # nég=3 / pos=1) : c'est le seul cas qui donne au contour ∂ quelque chose à
  # distinguer. Les autres stores ont une courbure sentinelle, ou entièrement
  # négative — et l'axe ∂ y est alors légitimement absent.
  "Linux drivers-net (ricci)=/tmp/kv-linux-driversnet.obrain"
)
# arrêt robuste (anciens processus éventuellement encore vivants)
for p in $(pgrep -f 'obrain viz serve' 2>/dev/null); do kill -9 "$p" 2>/dev/null || true; done
fuser -k ${PORT}/tcp 2>/dev/null || true
# ── ATTENDRE QUE LE SERVEUR AIT RENDU SES BASES ─────────────────────────────
# Défaut mesuré : avec un simple `sleep 1`, l'entretien de démarrage trouvait
# encore 8 bases « tenues » par le serveur PRÉCÉDENT, en train de mourir — un
# processus qui rend douze bases mmapées met plus d'une seconde. La fenêtre
# d'exclusivité était donc gaspillée, et c'est précisément celle que l'entretien
# existe pour occuper. On attend la sortie EFFECTIVE, port libéré ET plus aucun
# processus.
for _ in $(seq 1 60); do
  ss -ltnp 2>/dev/null | grep -q ":${PORT} " || break
  sleep 0.5
done
for _ in $(seq 1 60); do
  # Crochet : `pgrep -f 'obrain viz serve'` matcherait AUSSI la ligne de commande
  # de ce script (le piège documenté du dépôt, rencontré trois fois déjà).
  pgrep -f 'obrain vi[z] serve' >/dev/null 2>&1 || break
  sleep 0.5
done
cd "$(dirname "$0")/.."

# ── L'ENTRETIEN VIT ICI, PAS DANS UNE COMMANDE À RETENIR ────────────────────
#
# Les réparations (colonnes de degrés, enregistrements corrompus) exigent une
# ouverture EXCLUSIVE, donc que plus personne ne tienne les bases. Le seul moment
# où c'est vrai sans geste humain, c'est **maintenant** : le service vient d'être
# arrêté, il n'est pas encore relancé. Déléguer l'entretien au redémarrage, c'est
# la seule façon qu'il ait lieu — le reste est de la bonne volonté d'opérateur.
#
# `OBRAIN_SANS_ENTRETIEN=1` le contourne (diagnostic, mesure de latence).
# `OBRAIN_REPAIR_APPLY=1` autorise en plus la réparation des enregistrements
# corrompus, qui ÉCRIT dans les bases — diagnostic seul par défaut.
if [ "${OBRAIN_SANS_ENTRETIEN:-0}" != "1" ] && [ -x scripts/entretien-bases.sh ]; then
  bash scripts/entretien-bases.sh || true
fi

# ── ARMEMENT IAM : ICI, PARCE QUE C'EST ICI QUE LE REDÉMARRAGE PASSE ────────
# L'armement ne peut pas vivre dans un lancement à la main : il a été perdu
# QUATRE fois aujourd'hui, à chaque relance du dashboard. Ce script est le seul
# chemin de redémarrage — c'est donc ici que la posture doit être écrite.
#
# Défaut : ARMÉ. Les identités vivent dans un magasin DÉDIÉ et ÉCRIVABLE
# (`iam.obrain`, jamais une base servie : sur une base servie — ouverte en
# lecture seule — connexion et déconnexion deviennent décoratives).
#
# Pour désarmer explicitement :   OBRAIN_IAM=0 bash scripts/vizserve.sh
# (un `OBRAIN_IAM` explicite fait toujours foi ; `1`/`true`/`on`/`oui` arment).
OBRAIN_IAM="${OBRAIN_IAM:-1}"
OBRAIN_IAM_ACCOUNT="${OBRAIN_IAM_ACCOUNT:-obrain}"
OBRAIN_IAM_STORE="${OBRAIN_IAM_STORE:-$DATA/iam.obrain}"
export OBRAIN_IAM OBRAIN_IAM_ACCOUNT OBRAIN_IAM_STORE

# ── INDEX DE TRIGRAMMES : CONSTRUIRE ET PERSISTER (2026-09-14, mesuré) ───────
# Sans ces deux réglages, la construction en mémoire n'a JAMAIS lieu :
# `trigrammes_des_la` vaut `u32::MAX` par défaut, depuis que cette construction
# a rendu 0 là où le balayage rend 30 825 (un store vivant écrit pendant la
# lecture de l'instantané). Conséquence : chaque `CONTAINS` balaie.
#
# MESURÉ sur 348 000 nœuds, forme `MATCH (n) WHERE n.qualified_name CONTAINS
# 'iam'` : 59,6 ms (max 154,7) sans, 0,55 ms (max 1,3) avec la persistance.
# `DES_LA=1` construit dès la première demande (0,26 à 1,37 s, une fois) et
# `PERSISTER=1` écrit le fichier ancré : le processus suivant ne construit plus
# rien, et la fenêtre de rejeu garde l'index honnête après une écriture.
OBRAIN_TRIGRAMMES_DES_LA="${OBRAIN_TRIGRAMMES_DES_LA:-1}"
OBRAIN_TRIGRAMMES_PERSISTER="${OBRAIN_TRIGRAMMES_PERSISTER:-1}"
export OBRAIN_TRIGRAMMES_DES_LA OBRAIN_TRIGRAMMES_PERSISTER

# ── PRÉFIXE = INTERVALLE (réécriture du planificateur) ───────────────────────
# `n.prop STARTS WITH 'p'` devient l'intervalle `[p, succ(p))` servi par l'index
# ordonné. Mesuré sur le store vivant : 47,9 ms → 1,5 ms sur un préfixe de
# `qualified_name` (et ×100 à ×250 sur des clefs discriminantes). La garde
# `index_ordonne_disponible` empêche la réécriture quand la série n'est pas là.
# `OBRAIN_PREFIXE_INTERVALLE=0` la coupe — l'échappatoire reste, pour mesurer.
OBRAIN_PREFIXE_INTERVALLE="${OBRAIN_PREFIXE_INTERVALLE:-1}"
export OBRAIN_PREFIXE_INTERVALLE
if [ "$OBRAIN_IAM" = "0" ]; then
  echo "IAM : désarmé explicitement (OBRAIN_IAM=0)"
elif [ -d "$OBRAIN_IAM_STORE" ]; then
  echo "IAM : armé — identités dans $OBRAIN_IAM_STORE (se connecter sur /login)"
else
  echo "IAM : armement DEMANDÉ mais magasin d'identités ABSENT ($OBRAIN_IAM_STORE)"
  echo "      → le serveur se dira NON ARMÉ au démarrage ; l'amorcer avec :"
  echo "      cargo run --release -p obrain-cli --features iam --example amorcer_iam -- \\"
  echo "        $OBRAIN_IAM_STORE obrain <espace> <utilisateur> [<mot de passe>]"
fi

# construction de la ligne de commande serveur
CMD=(./target/release/obrain viz serve --host "$HOST" --port "$PORT" --preset-default "$DEFAULT")
for pr in "${PRESETS[@]}"; do CMD+=(--preset "$pr"); done
# La base synaptique de l'agent alimente /api/viz/weave (vue Cosmos).
[ -f "$SYNAPSE/substrate.meta" ] && CMD+=(--synapse-store "$SYNAPSE")
nohup "${CMD[@]}" > /tmp/vizserve.log 2>&1 < /dev/null &
SVPID=$!
echo "viz serve lancé (pid $SVPID) — log : /tmp/vizserve.log"
sleep 1.5
curl -s -o /dev/null -w 'racine: %{http_code}\n' "http://$HOST:$PORT/" || true

# ── Préchauffage des caches disque (parallèle, ne bloque pas le serveur) ──
echo "préchauffage des agrégats…"
PIDW=()
for pr in "${PRESETS[@]}"; do
  path="${pr#*=}"
  label="${pr%%=*}"
  (
    curl -s -o /dev/null --get --data-urlencode "store=$path" "http://$HOST:$PORT/api/viz/modules"
    curl -s -o /dev/null --get --data-urlencode "store=$path" "http://$HOST:$PORT/api/viz/overview"
    # ── `arcs` N'EST PLUS PRÉCHAUFFÉ ────────────────────────────────────────
    # Il l'était pour les labels *Linux*|*Obrain*|*Runtime* — c'est-à-dire les
    # TROIS bases lourdes (1,5 M nœuds / 4,8 M arêtes, 372 k, 316 k). Et cette
    # boucle tourne EN PARALLÈLE (`( … ) &`) : trois agrégats `arcs` concurrents,
    # chacun allouant le groupement de millions d'arêtes. Résultat constaté : le
    # lancement a été tué (OOM) et le contrôle de santé a répondu `000`.
    #
    # Ce préchauffage n'apporte qu'un gain de PREMIÈRE visite : l'endpoint tient
    # ensuite un cache durable (~1 ms). On préfère une première visite lente à un
    # serveur qui ne démarre pas. Réactivable si besoin :
    if [ "${OBRAIN_PREHEAT_ARCS:-0}" = "1" ]; then
      curl -s -o /dev/null --max-time 180 --get --data-urlencode "store=$path" \
        --data-urlencode "top=48" --data-urlencode "limit=320" "http://$HOST:$PORT/api/viz/arcs" || true
    fi
  ) &
  PIDW+=($!)
done
for p in "${PIDW[@]}"; do wait "$p"; done

# ── `arcs` : PRÉCHAUFFÉ, MAIS UN PAR UN ─────────────────────────────────────
# Il avait été retiré du préchauffage parce qu'il provoquait un OOM. La cause
# n'était pas `arcs` lui-même mais le fait que la boucle ci-dessus tourne EN
# PARALLÈLE : trois agrégats `arcs` concurrents sur 1,5 M / 372 k / 316 k nœuds,
# chacun groupant des millions d'arêtes. MESURÉ à froid : 22,6 s pour un seul.
#
# Maintenant que la fraîcheur se lit sur le manifeste du store (et non sur le
# dossier, qui change à chaque écriture), ce préchauffage n'est payé QU'UNE FOIS
# et tient — y compris à travers un redémarrage. On le fait donc SÉQUENTIELLEMENT,
# avec une borne de temps : la mémoire ne s'additionne plus.
for pr in "${PRESETS[@]}"; do
  label="${pr%%=*}"; path="${pr#*=}"
  case "$label" in
    *Linux*|*Obrain*|*Runtime*)
      echo "  arcs : $label (séquentiel, borné)"
      curl -s -o /dev/null --max-time 300 --get --data-urlencode "store=$path" \
        --data-urlencode "top=120" --data-urlencode "limit=400" \
        "http://$HOST:$PORT/api/viz/arcs" || true
      ;;
  esac
done
echo "préchauffage terminé — caches disque prêts."
