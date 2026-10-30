# Non-persistance ~1 h (2026-09-05, 20:13 → 21:21:33) — diagnostic + blindage « que ça n'arrive plus »

**Constat** (session-6eabd7a9, runtime-dev) : le journal de session s'est figé à
**20:13:08** alors que la conversation/les rounds continuaient (commits jusqu'à
21:11) ; aucun writer `runtime-dev` vivant entre ~20:13 et **21:21:33** ; au
retour du writer, **rattrapage confirmé** : 148 327 événements de la fenêtre
rejoués (max 21:22:36, 25 messages utilisateur) — rien n'est perdu à terme
mais la base a menti (état figé, weave/liens absents) pendant ~1 h.

## 1. Faits établis (preuves)

- Journal `runtime-dev/substrate.journal.session-6eabd7a9*` : 835 k événements,
  max horodaté **20:13:08** pendant la fenêtre ; mtime n'avançait pas ;
- `ps` : aucun process `obrain bridge`/writer sur runtime-dev de ~20:13 à
  21:21:33 ; backups binaires prod 20:11:14 et 21:11:08 ; process GUI web
  `--port 3083` (re)parti à **21:15:10** ; writer runtime-dev respawné à
  **21:21:33** (PID 1142959) ; après quoi replay → convergence (21:22:36+),
  et `obrain-live-sync` (import CBM) suit (fichier live 21:23:48) ;
- Harness `graph-writer.ts` (session-persistence) : GraphWriter = queue
  mémoire + flush 500 ms + **retry/re-spawn au prochain flush si bridge mort**
  (doc : « Jamais d'impact sur la durabilité ») → mécanisme correct SI le
  process propriétaire reste vivant.

## 2. Questions racine restantes (à trancher dans le chantier)

1. Qui possède l'instance `GraphWriter` (process web 3083 ? worker session ?) :
   sa mort ⇒ queue mémoire perdue ⇒ d'où viennent les événements rejoués
   (replay depuis le store de session du harness au boot ?) — localiser le
   boot/replay dans session-persistence.
2. Pourquoi aucun writer entre ~20:13 et 21:15 (process propriétaire mort ?) et
   pourquoi 21:15:10 → 21:21:33 (+6 min) avant le respawn (premier flush après
   événement ? idle ? backoff ?).
3. Où étaient les événements 20:13-21:15 pendant l'absence (buffer du process
   mort = perdu ? source durable du harness rejouée ?) — la réponse détermine
   la garantie réelle (perte possible = queue du process mort).

## 3. Blindage « que ça n'arrive plus » (design, DoD 8 cases à détailler à l'implémentation)

Couches indépendantes (la garantie ne repose sur aucune couche seule) :

- **C1 — Respawn fiable + health-check du writer (harness)** : garde qui vérifie
  que le writer répond (ping) et que `dernier seq journal` avance ; re-spawn
  immédiat si mort, backoff borné, alarme après N échecs. Test : tuer le
  writer → respawn < seuil + queue vidée.
- **C2 — Replay garanti au (re)démarrage (harness)** : au boot, re-import
  idempotent (par `(session, seq)`, marqueur `last_imported_seq`) de TOUTE
  session active depuis le store de session du harness — rend le rattrapage
  déterministe même si la queue d'un process mort est perdue. Test :
  « crash avec queue non vidée » → redémarrage → counts store == source.
- **C3 — Sentinelle de continuité (moteur, Rust)** : `obrain continuity
  <store> [--stale-after-min N]` (LECTURE) — liste les sessions dont le
  journal zone n'a pas reçu d'événement depuis N minutes + dernier seq ;
  sortie JSON stable, exit code 1 si périmé → branchable sur un timer
  systemd/cron par l'opérateur (pas de logique en JS).
- **C4 — Porte post-déploiement** : après chaque bascule binaire (prod/dev),
  vérifier par `continuity` : writer vivant + max horodaté journal < seuil +
  counts store == attendu (rejouable, chiffrée au ledger).

Exigences habituelles (additif, format inchangé, portes vertes engine/core/
substrate, un commit par correctif, pas de JS pour la logique, R6 : pas de
restart du service hébergeant la session pendant l'implémentation).

## 4. Cause racine C1/C2 localisée (code, worktree graph-writer)

Lecture du code (round 2) :
- `packages/session/session-persistence/src/graph-writer.ts` (~164 l.) — le
  GraphWriter pousse vers le store :
  - `flush()` (l.75-120) : **fire-and-forget SANS ACCUSÉ** — écrit la commande
    `import_events` sur stdin, drainne stdout (`rl.on('line', ...)` vide) et
    **ignore les réponses** ; garde 2 s de timeout sur l'écriture seule. Un
    `import_events` qui RÉPOND une erreur (store verrouillé, ouverture
    échouée…) laisse les événements **perdus en silence** (ni re-queue, ni
    alerte) ;
  - re-queue uniquement si `ensureProc()===null` ; un child mort dont l'event
    `exit` n'a pas encore basculé `this.proc` → `ensureProc` le réutilise et
    les écritures partent dans le vide ;
  - pas de health-check (aucun ping/réponse lue) ;
- `coordinator.ts` (l.714-716) : après le commit JSONL (vérité durable), push
  `graphWriter.append` en try/catch vide — **aucun re-push au boot** : si la
  queue d'un process est perdue, la reprise dépend du hasard (HMR/restart).
  Le rattrapage observé le 21:21:33 vient d'une re-synchronisation ultérieure,
  pas d'un mécanisme garanti.
- `session-persistence-jsonl/src/index.ts` (l.123+) : la config du writer est
  construite ici (DSH_GRAPH_WRITER=1 → GraphWriter obrain) — la source durable
  est le **JSONL** (backend), qui EST rejouable (readFrom(fromSeq)).

## 5. Spec d'implémentation C1/C2 — **C1 LIVRÉ (round 3, harness `b247983b63`)** : graph-writer.ts acquitté + auto-guérison, spec 3/3, suite 141/141. **C2 LIVRÉ (round 4, harness `b03be8a5c`)** : re-push idempotent JSONL→graphe au premier append, suites 380/380 ; spec E2E validée (harness `3e67901fe8`) : premier flush = log complet (n=3), append suivant = delta (n=1), un seul re-push par session (backend JSONL réel + fake bridge). Cas « restart multi-instances » couvert par le mécanisme + la spec C1 (mort→respawn, queue conservée).

**C1 — graph-writer.ts : canal acquitté + auto-guérison**
1. `ensureProc` crée un `readline` sur stdout avec une **file de répondeurs**
   (une requête en vol) ; à chaque ligne → parse JSON : `ok =
   !r.isError && r.structuredContent?.status === 'ok'` ; résout le répondeur.
2. `flush()` : pour chaque session, écrit la commande puis **attend la
   réponse** (timeout 5 s) ; `false` (erreur/timeout/child close) →
   **re-queue du lot** + warn throttle (ex. 1 warn/30 s) ; `true` → lot livré.
3. Erreur d'écriture (EPIPE) dans le callback `stdin.write` → marque le child
   mort (`this.proc=null`, kill) + re-queue → respawn au flush suivant.
4. `child.on('exit')` déjà présent → conserve ; y ajouter « fail pending ».
5. Test : fake bridge (fichier exécutable, pattern `fake-bridge.ts`) qui
   (a) répond `{status:'ok'}` et compte les commandes → la queue se vide ;
   (b) répond une erreur → la queue **reste** (rien de perdu) ;
   (c) meurt en EPIPE au milieu → requeue + respawn (spawnCount ≥ 2).

**C2 — coordinator.ts : re-push JSONL→graphe au boot (idempotent)**
1. Au premier append (ou à l'init quand des sessions live existent) : pour
   chaque session du backend, `readFrom(id, 0)` et `graphWriter.append(id,
   events)` en lot — le serveur (`import_events`) est idempotent par
   `(session, seq)` via `last_imported_seq` : re-pousser l'historique ne crée
   aucun doublon et **rattrape toute queue perdue**.
2. Borne : un seul re-push par session au démarrage (flag mémoire), coût
   O(historique) amorti une fois.
3. Test : simuler « queue perdue » (GraphWriter sans flush réussi) → init →
   les events JSONL sont tous présents côté graphe (fake bridge compte les
   `import_events` et vérifie les seq).

**Ordre** : C1 (round suivant) → C2 → tests des deux dans le repo harness
(`pnpm vitest run` des specs ciblées) → C4 porte post-déploiement (utiliser
`obrain continuity` + counts) → déploiement avec l'utilisateur (R6 : pas de
restart du service hébergeant la session pendant l'implémentation).
