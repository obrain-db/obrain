#!/usr/bin/env bash
# ── PORTE D'ACCEPTATION DU 8432 ─────────────────────────────────────────────
#
# Un seul point d'entrée qui répond à : « le port 8432 est-il au goût du jour,
# et est-ce PROUVÉ ? » Il enchaîne, dans l'ordre où les choses peuvent échouer :
#
#   1. les DEUX builds qui comptent (le binaire avec `ai`, et le dashboard —
#      `tsc` est une PORTE dans ce dépôt, voir le commit 108d2c25) ;
#   2. le PROTOCOLE, sur une instance ISOLÉE (8433) : traversée de chemin,
#      compression, keep-alive, HEAD, Date, ETag/304, SSE ;
#   3. les ENDPOINTS : présence, code, latence, chaud ET froid ;
#   4. la CHAÎNE MULTI-DIMENSION à l'écran (`scripts/projection-check.mjs`) ;
#   5. les FRÈRES ET SŒURS à la sélection (`scripts/fratrie-check.mjs`).
#
# Pourquoi une instance isolée : deux sessions et plusieurs agents travaillent
# dans cet arbre. `scripts/vizserve.sh` tue par motif et reprend 8432 ; mesurer
# dessus pendant qu'un autre la relance, c'est mesurer l'ANCIEN binaire en
# croyant mesurer le nouveau — constaté deux fois, d'où `vizserve-isole.sh`.
#
# Usage :
#   scripts/acceptation-8432.sh                 # tout, instance isolée 8433
#   scripts/acceptation-8432.sh --sans-build    # sauter les builds (déjà faits)
#   scripts/acceptation-8432.sh --vif 8432      # vérifier un service DÉJÀ lancé
#
# Sortie : une ligne PASS/FAIL par vérification, un verdict, et un code de
# sortie non nul dès qu'une vérification échoue. Aucun « je pense que ».
set -uo pipefail
cd "$(dirname "$0")/.."

ROOT="$(pwd)"
ISOLE_PORT=8433
VIF=0
BUILD=1
while [ $# -gt 0 ]; do
  case "$1" in
    --sans-build) BUILD=0; shift ;;
    --vif) VIF="$2"; shift 2 ;;
    *) echo "option inconnue : $1" >&2; exit 2 ;;
  esac
done

KO=0
# ── TROISIÈME ÉTAT : « NON MESURÉ » (2026-09-16) ─────────────────────────────
# Un contrôle qui n'a pas pu mesurer n'est PAS un contrôle qui a échoué, et il
# n'est pas non plus un contrôle qui a réussi. Les confondre dans un seul
# compteur fait mentir la porte dans les deux sens : elle accuse le PRODUIT d'un
# défaut qu'elle n'a pas observé (« la fratrie n'est pas prouvée » alors que le
# contrôle a écrit « le contrôle NE MESURE PAS »), ou elle laisse croire que tout
# va bien alors que rien n'a été vérifié.
# Ce compteur-là ne rend donc PAS le verdict vert : il le rend seulement EXACT.
NM=0
ETAPE=0
titre() { ETAPE=$((ETAPE+1)); printf '\n── %d. %s\n' "$ETAPE" "$1"; }
ok()    { printf 'PASS  %s\n' "$1"; }
non()   { printf 'FAIL  %s\n' "$1"; KO=$((KO+1)); }
nm()    { printf 'NON MESURÉ  %s\n' "$1"; NM=$((NM+1)); }
note()  { printf '      %s\n' "$1"; }

# ── 1. LES BUILDS ───────────────────────────────────────────────────────────
titre "Compilations (c'est ici que tout le reste devient vrai)"
if [ "$BUILD" = 1 ]; then
  # `--features ai` n'est pas une coquetterie : un build SANS `ai` retire
  # `vector-index`, `text-index` et `hybrid-search`, et la recherche sémantique
  # devient « indisponible dans ce déploiement » (constaté, en-tête de
  # scripts/vizserve.sh). Le défaut du binaire de déploiement est CELUI-CI.
  if cargo build --release -p obrain-cli --features ai >/tmp/acceptation-cargo.log 2>&1; then
    ok "cargo build --release -p obrain-cli --features ai"
  else
    non "cargo build — voir /tmp/acceptation-cargo.log"
    tail -20 /tmp/acceptation-cargo.log | sed 's/^/      /'
  fi
  # ── POURQUOI LE BUILD VIT DANS UN RÉPERTOIRE TEMPORAIRE (2026-09-16) ───────
  # `vite.config` a `outDir: "dist"`, et `packages/obrain-dashboard/dist` est un
  # LIEN SYMBOLIQUE vers `$UI_DIR/current` — l'UI SERVIE EN PRODUCTION. Un
  # `npx vite build` nu écrit donc DANS l'interface vivante : au mieux il échoue
  # sur les permissions d'une release (elles sont en 0555, et cette porte
  # rapportait donc un FAIL permanent qui n'était pas un défaut du dashboard), au
  # pire il la corrompt en pleine session utilisateur — et c'est un « au pire »
  # qui ne tient qu'à un bit de permission.
  #
  # Le chemin sanctionné pour PUBLIER une UI reste `scripts/ui-deploy.sh`, qui
  # construit dans un répertoire temporaire puis bascule `current`. Cette porte,
  # elle, doit seulement prouver que le dashboard CONSTRUIT : elle reprend donc la
  # même précaution, et le dit — prouver la construction n'est pas publier.
  #
  # `tsc -b` n'est pas concerné : le tsconfig porte `noEmit: true`, il n'écrit rien.
  TMP_UI="$(mktemp -d /tmp/acceptation-ui-XXXXXX)"
  if (cd packages/obrain-dashboard && npx tsc -b >/tmp/acceptation-tsc.log 2>&1 \
      && npx vite build --outDir "$TMP_UI" --emptyOutDir >>/tmp/acceptation-tsc.log 2>&1); then
    ok "dashboard : tsc -b + vite build (dans $TMP_UI, HORS de l'UI servie)"
    note "publier l'UI est un autre geste : scripts/ui-deploy.sh"
    rm -rf "$TMP_UI"
  else
    non "dashboard : build — voir /tmp/acceptation-tsc.log"
    tail -20 /tmp/acceptation-tsc.log | sed 's/^/      /'
  fi
else
  note "builds sautés (--sans-build)"
fi

# ── 2. L'INSTANCE À MESURER ─────────────────────────────────────────────────
titre "Instance à mesurer"
if [ "$VIF" != 0 ]; then
  BASE="http://192.168.1.103:$VIF"
  note "service déjà lancé : $BASE (on ne démarre rien)"
else
  BASE="http://127.0.0.1:$ISOLE_PORT"
  note "instance isolée demandée sur $BASE"
  # ── NE PAS TUER UNE INSTANCE QUI TOURNE DÉJÀ ──────────────────────────────
  # `vizserve-isole.sh` arrête par PID du port. Si un autre agent mesure sur ce
  # port en ce moment, le relancer lui arracherait son serveur en pleine passe
  # — et il publierait des chiffres pris sur un binaire qui n'est plus le sien.
  # On RÉUTILISE donc une instance qui répond, on n'en démarre une que si le
  # port est muet.
  if curl -s -o /dev/null --max-time 3 "$BASE/api/stores"; then
    note "une instance écoute déjà — on la réutilise (aucun arrêt)"
  else
    scripts/vizserve-isole.sh --port "$ISOLE_PORT" >/tmp/acceptation-isole.log 2>&1 || true
  fi
  for _ in $(seq 1 60); do
    curl -s -o /dev/null --max-time 2 "$BASE/api/stores" && break
    sleep 1
  done
  if curl -s -o /dev/null --max-time 5 "$BASE/api/stores"; then
    ok "instance isolée répond sur $BASE"
  else
    non "instance isolée MUETTE — voir /tmp/acceptation-isole.log"
    tail -20 /tmp/acceptation-isole.log | sed 's/^/      /'
  fi
fi
# Le binaire RÉELLEMENT servi : sans ce contrôle, on peut mesurer l'ancien et
# l'appeler « après » (c'est arrivé deux fois dans ce dépôt).
note "binaire : $(ls -la --time-style=+%H:%M target/release/obrain | awk '{print $6, $7}')"

# ── 3. LE PROTOCOLE ─────────────────────────────────────────────────────────
titre "Protocole (14 échecs sur 15 avant ce chantier)"
if [ -f scripts/protocole-check.mjs ]; then
  if node scripts/protocole-check.mjs "$BASE" 2>&1 | sed 's/^/      /'; then
    ok "scripts/protocole-check.mjs — tout passe"
  else
    non "scripts/protocole-check.mjs — au moins un échec (détail ci-dessus)"
  fi
else
  non "scripts/protocole-check.mjs absent"
fi

# ── 4. LES ENDPOINTS ────────────────────────────────────────────────────────
titre "Endpoints (présence, code, latence)"
if [ -f scripts/endpoints-check.mjs ]; then
  if node scripts/endpoints-check.mjs "$BASE" >/tmp/acceptation-endpoints.log 2>&1; then
    ok "scripts/endpoints-check.mjs — verdict global OK"
  else
    non "scripts/endpoints-check.mjs — voir /tmp/acceptation-endpoints.log"
  fi
  grep -aE '^\s*(ERREUR|LENT|ABSENT)' /tmp/acceptation-endpoints.log | head -12 | sed 's/^/      /'
else
  non "scripts/endpoints-check.mjs absent"
fi

# ── 5. LA CHAÎNE MULTI-DIMENSION, À L'ÉCRAN ─────────────────────────────────
titre "Chaîne multi-dimension (requête → axes → sélection → graphe)"
if [ -f scripts/projection-check.mjs ]; then
  if node scripts/projection-check.mjs "$BASE" 2>&1 | tail -30 | sed 's/^/      /'; then
    ok "scripts/projection-check.mjs"
  else
    non "scripts/projection-check.mjs — la chaîne n'est pas prouvée"
  fi
else
  non "scripts/projection-check.mjs absent — la chaîne multi-dimension n'est PAS prouvée"
fi

# ── 6. LES FRÈRES ET SŒURS, À L'ÉCRAN ───────────────────────────────────────
titre "Frères et sœurs à la sélection"
# ⚠ LA PORTE DOIT DÉMARRER LE NAVIGATEUR ELLE-MÊME (ajouté le 2026-09-15).
# `fratrie-check.mjs` ne lance AUCUN navigateur : il se CONNECTE à un CDP déjà
# ouvert (`argv[3]`, défaut http://127.0.0.1:9248). Jusqu'ici la porte appelait
# le contrôle sans lui donner de navigateur : il mourait en `ECONNREFUSED` et le
# verdict « la fratrie n'est pas prouvée » accusait le PRODUIT alors qu'il
# accusait un harnais incomplet — constaté, puis corrigé ici.
if [ -f scripts/fratrie-check.mjs ]; then
  CHROME="${CHROME:-$HOME/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome}"
  CDP_PORT_FRATRIE=9248
  CDP_PID=""
  if curl -s -m 3 "http://127.0.0.1:$CDP_PORT_FRATRIE/json/version" >/dev/null 2>&1; then
    note "chrome CDP déjà en service sur $CDP_PORT_FRATRIE — réutilisé"
  elif [ -x "$CHROME" ]; then
    "$CHROME" --headless=new --no-sandbox --disable-dev-shm-usage \
      --use-angle=swiftshader --enable-unsafe-swiftshader \
      --remote-debugging-port=$CDP_PORT_FRATRIE \
      --user-data-dir="/tmp/acceptation-cdp-$CDP_PORT_FRATRIE" \
      --window-size=1440,900 about:blank >/tmp/acceptation-chrome.log 2>&1 &
    CDP_PID=$!
    for _ in $(seq 1 20); do
      curl -s -m 2 "http://127.0.0.1:$CDP_PORT_FRATRIE/json/version" >/dev/null 2>&1 && break
      sleep 1
    done
    note "chrome CDP lancé par la porte (pid $CDP_PID, port $CDP_PORT_FRATRIE)"
  else
    note "chromium introuvable ($CHROME) — la fratrie sera NON APPLICABLE"
  fi
  if curl -s -m 3 "http://127.0.0.1:$CDP_PORT_FRATRIE/json/version" >/dev/null 2>&1; then
    # ── « NON MESURÉ » N'EST PAS « ÉCHEC PROUVÉ » (2026-09-16) ─────────────
    # Le contrôle distingue lui-même les deux : quand aucune cible exploitable
    # n'existe sur l'instance, il écrit « aucune cible exploitable sur cette
    # instance — le contrôle NE MESURE PAS ». La porte transformait ce
    # non-mesurage en « la fratrie n'est pas prouvée » : elle accusait donc le
    # PRODUIT d'un défaut qu'elle n'avait jamais observé — le même travers que
    # celui corrigé juste au-dessus (accuser le produit d'un harnais incomplet),
    # sous une autre forme.
    #
    # La cause la plus fréquente ici : la porte RÉUTILISE une instance déjà
    # lancée, dont la liste de presets peut être réduite. `Linux e1000` n'existe
    # alors pas, et le contrôle n'a rien à sélectionner. Ce qui est impossible,
    # c'est de MESURER — pas l'objet mesuré qui serait défectueux.
    #
    # Et un non-mesuré ne rend pas le verdict vert : on ne remplace pas un faux
    # échec par un faux succès.
    SORTIE_FRATRIE="$(node scripts/fratrie-check.mjs "$BASE" "http://127.0.0.1:$CDP_PORT_FRATRIE" 2>&1)"
    CODE_FRATRIE=$?
    printf '%s\n' "$SORTIE_FRATRIE" | tail -30 | sed 's/^/      /'
    if [ "$CODE_FRATRIE" = 0 ]; then
      ok "scripts/fratrie-check.mjs"
    elif printf '%s' "$SORTIE_FRATRIE" | grep -q 'NE MESURE PAS'; then
      nm "scripts/fratrie-check.mjs — aucune cible exploitable sur CETTE instance (voir le détail ci-dessus). Forcer la cible : FRATRIE_STORE=… FRATRIE_CENTRE=…"
    else
      non "scripts/fratrie-check.mjs — la fratrie n'est pas prouvée"
    fi
  else
    nm "scripts/fratrie-check.mjs — aucun navigateur CDP disponible : harnais incomplet, ce n'est pas un défaut du produit"
  fi
  # Le navigateur lancé par la porte est refermé ; celui qu'on a trouvé ne l'est pas.
  [ -n "$CDP_PID" ] && kill -TERM "$CDP_PID" 2>/dev/null
else
  non "scripts/fratrie-check.mjs absent — la fratrie n'est PAS prouvée"
fi

# ── VERDICT ─────────────────────────────────────────────────────────────────
printf '\n══════════════════════════════════════════════════════════════════\n'
if [ "$KO" = 0 ] && [ "$NM" = 0 ]; then
  printf 'ACCEPTATION : TOUT PASSE (%s)\n' "$BASE"
elif [ "$KO" = 0 ]; then
  printf 'ACCEPTATION : %d NON MESURÉ(S) — la porte ne peut PAS conclure\n' "$NM"
  printf '  aucun échec constaté, mais un contrôle qui n'"'"'a pas mesuré ne prouve rien\n'
else
  printf 'ACCEPTATION : %d ÉCHEC(S)' "$KO"
  if [ "$NM" -gt 0 ]; then printf ' et %d NON MESURÉ(S)' "$NM"; fi
  printf '\n'
fi
# Trois verdicts, pas deux : un non-mesuré n'est ni un succès ni un échec prouvé —
# et il laisse le code de sortie NON NUL. Un contrôle qu'on n'a pas pu faire ne
# doit jamais pouvoir passer pour un contrôle qui a réussi.
printf 'Rappel — mesurer le service de l'"'"'utilisateur se fait ENSUITE :\n'
printf '  systemctl --user restart obrain-viz.service\n'
printf '  node scripts/protocole-check.mjs http://192.168.1.103:8432\n'
printf '══════════════════════════════════════════════════════════════════\n'
if [ "$KO" -gt 0 ] || [ "$NM" -gt 0 ]; then exit 1; fi
exit 0
