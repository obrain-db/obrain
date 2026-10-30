#!/usr/bin/env bash
# ── BANC D'ESSAI AUTHENTIFIÉ POUR L'INTERFACE ───────────────────────────────
#
# Pourquoi il existe : le dashboard redirige maintenant vers `/login` quand
# personne n'est connecté (défaut mesuré le 2026-09-16 : l'accueil répondait 303
# vers /login). C'est mieux pour l'humain — un écran vide se lisait comme une
# panne — mais les portes d'interface ne testaient PLUS RIEN : sans session,
# elles mesuraient la page de connexion en croyant mesurer le dashboard.
#
# Ce banc donne aux portes un serveur ARMÉ et un JETON DÉJÀ ÉMIS, sans toucher
# aux données de l'utilisateur : magasin d'identités temporaire, base temporaire,
# port libre. Il imprime un JSON que les portes consomment (`--json`).
#
# Usage : scripts/banc-interface-authentifiee.sh [port] [--garder]
#   PORT 8441 par défaut — jamais 8432/8433 (services de l'utilisateur).
#
# Sortie (stdout) :
#   {"base":"http://127.0.0.1:8441","jeton":"obt_…","compte":"admin",
#    "mot_de_passe":"…","espace":"demo","iam":"/tmp/…","store":"/tmp/…",
#    "pid":1234}
#
# Les portes s'en servent ainsi :
#   BASE=$(scripts/banc-interface-authentifiee.sh 8441 | jq -r .base)
#   TOKEN=$(… | jq -r .jeton)
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO" || exit 1

PORT="${1:-8441}"
if [ "$PORT" = "8432" ] || [ "$PORT" = "8433" ]; then
  echo "refus : $PORT est un service de l'utilisateur. Choisis un port libre." >&2
  exit 2
fi

BIN="${OBRAIN_BIN:-./target/release/obrain}"
TAG="$$"
IAM="/tmp/banc-ui-iam-$TAG.obrain"
STORE="/tmp/banc-ui-store-$TAG.obrain"
LOG="/tmp/banc-ui-$TAG.log"
MDP="banc-$TAG-$RANDOM"

if [ ! -x "$BIN" ]; then
  echo "binaire absent : $BIN (construire : cargo build --release -p obrain-cli --features ai,iam)" >&2
  exit 2
fi

"$BIN" init "$STORE" >/dev/null 2>&1 || { echo "init base impossible" >&2; exit 2; }
"$BIN" init "$IAM" >/dev/null 2>&1 || { echo "init identités impossible" >&2; exit 2; }

# Le jeton est émis AVANT de démarrer le serveur : une fois le service lancé, il
# tient le verrou du magasin et le CLI ne peut plus y écrire.
AMORCAGE="$("$BIN" iam amorcer "$IAM" demo --utilisateur admin --mot-de-passe "$MDP" --json 2>/dev/null)"
JETON="$(printf '%s' "$AMORCAGE" | sed -n 's/.*"jeton"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
if [ -z "$JETON" ]; then
  echo "aucun jeton émis — l'IAM est-il compilé dans ce binaire ?" >&2
  rm -rf "$IAM" "$STORE"
  exit 2
fi

OBRAIN_IAM=1 OBRAIN_IAM_ACCOUNT=obrain OBRAIN_IAM_STORE="$IAM" \
OBRAIN_LLM_URL="${OBRAIN_LLM_URL:-http://192.168.1.7:9910}" \
# ── LE COMPTE DU BANC DOIT AVOIR UNE BASE À REGARDER ────────────────────────
# Défaut mesuré : sans base accordée, l'interface masque volontairement les
# contrôles qui n'ont pas de sens (le bouton de conversation n'est pas monté —
# il n'y aurait rien à lui demander). Toutes les portes échouaient alors pour une
# raison qui n'a rien à voir avec ce qu'elles mesurent : un banc qui ne
# ressemble pas à l'usage réel accuse l'interface de sa propre pauvreté.
# L'espace de l'administrateur EST « demo », et la base servie porte ce nom : le
# compte a donc une base, comme un vrai compte.
"$BIN" viz serve --host 127.0.0.1 --port "$PORT" --store "$STORE" \
  --preset-default demo --preset "demo=$STORE" --enrich-ricci 0 >"$LOG" 2>&1 &
PID=$!

# Attendre que l'accueil réponde — un serveur armé rend 200 (page ou /login),
# jamais 000.
for _ in $(seq 1 60); do
  c=$(curl -s -m 2 -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/" 2>/dev/null)
  [ "$c" != "000" ] && break
  sleep 1
done
if [ "$c" = "000" ]; then
  echo "le serveur n'écoute pas (voir $LOG)" >&2
  kill -9 "$PID" 2>/dev/null
  exit 2
fi

# Le jeton doit OUVRIR la porte : on le VÉRIFIE, on ne l'espère pas. Un banc qui
# rend un jeton refusé ferait échouer toutes les portes pour une mauvaise raison.
code=$(curl -s -m 5 -o /dev/null -w '%{http_code}' -H "Cookie: obrain_session=$JETON" "http://127.0.0.1:$PORT/api/stores")
if [ "$code" != "200" ]; then
  echo "le jeton ne passe pas ($code sur /api/stores) — banc inutilisable" >&2
  kill -9 "$PID" 2>/dev/null
  exit 2
fi

# ── LE SERVEUR DOIT SURVIVRE AU SCRIPT — SINON LE BANC NE SERT À RIEN ───────
# Erreur de ma première version : un `trap … EXIT` tuait le serveur dès que le
# script rendait la main, donc le fichier de sortie était écrit et le service
# déjà mort. Ici le banc RESTE EN AVANT et attend : il tue le serveur quand on
# l'interrompt (ou quand l'appelant tue le banc lui-même).
echo "banc UI : pid=$PID port=$PORT iam=$IAM store=$STORE log=$LOG" >&2
printf '{"base":"http://127.0.0.1:%s","jeton":"%s","compte":"admin","mot_de_passe":"%s","espace":"demo","iam":"%s","store":"%s","pid":%s}\n' \
  "$PORT" "$JETON" "$MDP" "$IAM" "$STORE" "$PID"

# ── ON NE SUPPRIME RIEN EN SORTANT : LE BANC SURVIT À SON APPELANT ─────────
# Erreur de ma première version, et elle m'a fait perdre deux mesures de suite :
# le `trap … EXIT` se déclenchait quand le SOUS-SHELL rendait la main, donc il
# tuait le serveur ET supprimait le magasin d'identités — le jeton devenait
# alors « invalide, révoqué ou expiré » sur le serveur suivant. Deux mesures
# contradictoires, dont une seule était vraie.
#
# Le banc est fait pour être lancé EN ARRIÈRE-PLAN par un appelant qui lira le
# fichier de sortie : il ne nettoie donc rien lui-même. C'est l'appelant qui tue
# le banc (son pid est dans la fiche) — et les fichiers temporaires portent un
# horodatage, donc ils sont identifiables.
trap 'echo "banc interrompu" >&2' INT TERM

wait "$PID"
