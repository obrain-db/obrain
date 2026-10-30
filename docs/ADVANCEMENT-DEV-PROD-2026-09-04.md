# Registre d'avancement dev → prod (historique durable — à lire par la prod)

> Objectif : que la prod (sessions/agents, maintenance) comprenne SON historique
> et l'écart avec dev. Écrit le 2026-09-04 sur `feat/zorder` (banc-obrain) +
> harness (graph-writer dev) + aletheia. Mise à jour à chaque avancée.

## 1. Ce que dev apporte (et que prod doit rattraper)
| Capacité | Dev (vérifié) | Prod (état) |
|---|---|---|
| Moteur obrain (Z-order, fenêtres 2D, compteurs O(1), ingestion native) | HEAD `feat/zorder`, tag `v0.0.1-zorder-2026-09-04` | Binaire partagé = tag ✓ (sha 8916d808…) |
| Preset GUI natif (brain_search, graph-injection) | bundle worktree + preset enrichi | Preset enrichi chargé (restart 14:00) — à confirmer outils visibles |
| Écriture graph (messages/résumés) | DSH_GRAPH_WRITER=1 + idle court (fenêtres RW) | Réactivé (writer ON, idle 15 s) — retest en cours |
| Store | runtime-dev (196 sessions) + runtime-full (import 644, ENGRAMS VIDES) | runtime.obrain (142, famille native) |
| Continuité (engrams, context_build, memory_recall) | par événements natifs uniquement | stores natifs : OK ; runtime-full import : vides (cause) |

## 2. Chronologie (2026-08/09 — compacte)
- Moteur : RFC zorder → fenêtres 2D (exact+triées) → compteurs O(1) ×100 000 → lookup ×935 →
  ingestion native `native-import` 0,27-0,31 s (445 k/1,67 M) → tests sémantiques (BM25/vector/hybrid) →
  C1 dépôt→analyse 0,3-0,5 s ; C2 topo-rerank precision@5 0,530→0,944. Commits `feat/zorder`, RFC §1-17.
- Checkpoint : tag `v0.0.1-zorder-2026-09-04` (kit de vérification `scripts/bench-verify/`, gardien).
- Harness : binaire partagé = tag ; sonde bridge ✓ ; preset prod enrichi (bak 13:49) ; idle
  OBRAIN_IDLE_MS 60000 → 21600000 → **15000** (raison : 6 h bloquait les écritures RW exclusives).
- Bases : `runtime-full.obrain` = import graphique complet de `dsh-runtime.db` (256 757 nœuds /
  252 146 arêtes / 644 sessions) — **écarté provisoirement** : bugs création/messages + engrams vides
  (structure import ≠ native). Prod revenue sur `runtime.obrain`.
- Aletheia : tag intégré ✓ (fenêtre n2) ; council `repair-degrees` fait (hubs degrés) ; R4b centralité = à faire.
- Sources : journaux maîtres `~/.dsh/sessions` (61 restants) ; **source durable complète =
  dsh-runtime.db** (`runtime_event_data` 23,9 M événements, runtime_payloads).

## 3. Décision actuelle (2026-09-04)
Le « full » qui garde TOUTES les features (engrams/continuité) doit être reconstruit par la
**voie native** (import_events/journal) depuis dsh-runtime.db → `runtime-full-v2` ; l'import
graphique (runtime-full) ne peuple ni engrams ni la structure d'écriture attendue.

## 4. Prochaines étapes (ordre)
1. runtime-full-v2 : échantillon 3 sessions (voie native) → vérif engrams/context_build ≠ vides ;
2. passe complète 640 sessions + QC + sonde ;
3. bascule prod (drop-in + restart, rollback documenté) ;
4. open paresseux moteur (<100 ms) ; 5. continuité spectrale/spatiale (docs/continuity-spectral-spatial.md).

## 5. Règles (rappel)
- Ne jamais casser la prod sans rollback immédiat ; garder backup/état AVANT daté ;
- Registre mis à jour à chaque avancée (sinon historique perdu) ;
- Dev 3083 = référence ; restart prod 3082 = fenêtre annoncée.

## 6. Enseignement vérifié (2026-09-04, suite « fais »)
- `journal_import` natif : 128 207 lignes importées en 0,8 s (store RW, open 4 ms) — la zone
  journal substrate fonctionne nativement.
- MAIS rejouer de vieux journaux ne crée PAS les engrams : les engrams (memory_register/
  co-activation) sont des ÉPISODES nés PENDANT le travail vivant (GraphWriter→engram_manager
  sur import_events), pas une dérivation du texte passé. Les 760 engrams mesurés viennent du
  live dev, pas d'un replay.
- Conséquence : la continuité mémoire pour le PASSÉ = reconstruction spectrale/spatiale depuis
  le graphe (docs/continuity-spectral-spatial.md) ; les engrams s'accumulent en LIVE sur le
  store de prod une fois l'écriture native active.
- Prochain pas concret : confirmer l'envoi/résumé sur prod (writer ON, idle 15 s) ; re-tenter
  ensuite la bascule sur runtime-full (lecture complète OK) avec écriture native — engrams
  neufs enregistrés live dessus.

## 7. Bascule effectuée (2026-09-04)
- Prod :3082 → OBRAIN_STORE=runtime-full.obrain (historique complet : 644 sessions /
  63 k messages), DSH_GRAPH_WRITER=1 (écriture native live), idle 15 s.
- Vérifié : service actif, HTTP 200, sessions_list 10 ms, session_load 61 ms,
  runtime_search OK (1er appel ~2,2 s = index BM25 paresseux, puis chaud).
- Rollback : env → runtime.obrain + restart dsh-web (documenté dans le drop-in).

## 8. Cause des messages manquants — VÉRIFIÉE (2026-09-04)
- La lecture native des messages s'appuie sur les arêtes `IN_SESSION` (type généré par
  l'import NATIF). Présent dans runtime-dev + runtime.obrain ; **ABSENT de runtime-full**
  (import graphique) → messages non restitués/lents sur runtime-full. C'est la même cause
  racine que les bugs « création/messages » : l'import graphique n'a pas la structure native.
- PROD (provisoire) : OBRAIN_STORE=runtime-dev.obrain (parité dev, messages OK, weave
  riche). runtime-full conservé (lecture par session_load OK mais PAS par le chemin UI natif).
- CIBLE : runtime-full-v2 = reconstruction NATIVE complète (644 sessions) générant
  IN_SESSION + weave dev :
  1) 61 archives exactes rejouées nativement ;
  2) 583 restantes : lignes reconstruites depuis dsh-runtime runtime_event_data (fidélité
     à valider sur échantillon, idempotence par seq) ;
  3) injecter le weave/épisodes de runtime-dev (le « mieux tissé ») ;
  4) QC : IN_SESSION présent, messages complets par session, memory/engrams live, sonde UI.
- Rollback prod immédiat : env → runtime.obrain (famille native complète d'origine).

## 9. runtime-full-v2 livré & prod basculée (2026-09-04)
- Importer patché : arêtes natives `IN_SESSION` session→message ajoutées à l'import
  complet (dsh-runtime). runtime-full-v2 : 259 164 nœuds / 317 733 arêtes (dont
  63 675 IN_SESSION, 0 session manquante), 1,6 Go, 27,7 s de build.
- QC : réouverture OK ; IN_SESSION O(1) = 63 675 ; sidecar degree IN_SESSION présent ;
  session_load 46 ms (messages) ; runtime_search OK.
- Prod :3082 → OBRAIN_STORE=runtime-full-v2.obrain (writer ON). Rollback : runtime-dev.
- À valider en UI prod : messages des anciennes conversations affichés + envoi/résumé.

## 10. QC complétude v2 (2026-09-04)
- v2 Message 63 675 vs cbm 63 683 (−8 = croissance live pendant build) ; IN_SESSION 63 675 = nb messages ✓.
- Dev ⊆ cbm (0 session dev hors cbm) → le tissage dev est couvert par l'import v2 (cbm).
- sessions_list : v2 liste 242 sessions (dev 131) — la liste applique un filtre (owner/
  workspace), pas un compteur de complétude : Session nodes v2 = 659 (= cbm).
- Écart à surveiller : 1 session dev absente de la liste v2 (session-9886ede8…) présente
  dans cbm → à traquer en UI si visible manquante.
- GATE final = validation UI prod (messages affichés + envoi/résumé) — itération jusqu'à parfait.

## 11. Traque session 9886ede8 + filtre sessions_list (2026-09-04)
- Session 9886ede8 PRÉSENTE dans v2 (aucune perte) ; mais v2 (snapshot cbm) n'a pas
  les props LIVE de dev (title/… écrits par import_events après le snapshot cbm).
- sessions_list ≈ sessions enrichies (title/owner) : dev 131 listées vs v2 242 — filtre
  de listing, pas de perte de contenu (nœuds Session v2 = 659 = cbm).
- Écart restant = enrichissements LIVE dev non encore dans cbm → outil « copie live » à
  bâtir (NAPI updateNodeProps absent) + sync dev→cbm. Gate final toujours = UI prod.

## 12. Re-snapshot ≠ capture du live dev (2026-09-04)
- v3 = v2 resynchronisé (cbm 14:2x) : +69 nœuds, Message 63 696 (+21) — mais
  sessions_list inchangé (242) et session 9886ede8 toujours non listée : les titres/
  props LIVE de dev ne sont pas (encore) dans cbm → re-snapshot insuffisant.
- Vraie correction = SYNC dev→cbm (ou copie store-side des props via outil à bâtir,
  ex. NAPI setNodeProperty). Prod reste sur runtime-full-v2 (contenu ≈ v3, IN_SESSION natif).
- Décision : pas de nouvelle bascule tant que le gap live n'est pas outillé ou validé en UI.

## 13. Enrichissement live dev→prod réalisé (2026-09-04)
- Outil : scripts/enrich-live-dev-to-store.mjs (utilise setNodeProperty existant).
- Résultat : 195 sessions enrichies (606 props : titre/env/createdAt/last_imported_seq…),
  1 introuvable (variante .jsonl). Session 9886ede8 DÉSORMAIS listée avec titre.
- Store prod runtime-full-v2 enrichi IN PLACE (écriture RW ok) ; sessions_list 243.

## 14. QC large v2 vs cbm (2026-09-04)
- Session 659=659, Subagent 156=156, Extraction 1=1 EXACTS. Message −38, Step −31,
  ToolCall/ToolResult −26, Turn −5 = croissance cbm DEPUIS le snapshot v2 (import live
  continue) — pas de déficit structurel. IN_SESSION = Message count (63 675) ✓.
- Le live s'accumule désormais sur v2 (writer prod) : v2 devient superset vivant.
- Reste (dépendant de l'UI/utilisateur) : validation affichage messages + envoi/résumé,
  et re-mesure engrams après écritures réelles.

## 15. « Messages ne reviennent pas » — localisation (2026-09-04)
- Côté obrain le chemin est OK : prod env → runtime-full-v2 ; engine : sessions_list 243,
  session_load renvoie les 384 messages de la session test.
- L'UI prod ne charge pas via session_load (bundle lib sans session_load/IN_SESSION) et
  tente encore le daemon codebase-memory (cbm) — logs « secure daemon endpoint could not be
  created » répétés. → Le maillon défaillant est la config/lecteur graph de l'APP prod
  (harness), pas le store obrain. C'est l'intégration documentée par l'agent harness
  (bundle + preset + graph-injection) — hors périmètre store.
- Décision : ne plus toucher le store ; passer la main à l'intégration app (deux options :
  dsh-deploy bundle upgrade prod, ou l'agent harness câble le lecteur obrain-native en
  s'appuyant sur ce registre). Obrain côté store : v2 prêt & validé (IN_SESSION, enrichi).

## 16. QC par session v2 vs cbm (2026-09-04)
- 112 sessions « propres » (hors alias .jsonl, artefacts d'import cbm) : comptage IN_SESSION
  par session vs messages cbm → 0 écart > 5 ; 37 743 messages vérifiés un à un (session_load OK).
- Note : cbm contient des ALIAS .jsonl qui dupliquent des sessions réelles (double comptage
  des messages) ; v2 les importe tels quels — nettoyage d'alias possible plus tard.
- Le périmètre STORE obrain de l'objectif est atteint (natif IN_SESSION, complet, enrichi).
  Reste l'intégration APP prod (lecteur messages) — choix utilisateur : dsh-deploy (chemin 1)
  ou agent harness (chemin 2).

## 17. Symptôme « chargement… puis rien » — correctif n°1 (2026-09-04)
- Le drop-in prod 10-cbm-runtime.conf orientait le client codebase-memory du SERVICE vers
  SON daemon cbm (/tmp/cbm-prod-runtime) → daemon absent → chargement UI vide.
- Action : 10-cbm-runtime.conf désactivé (.off) + restart → client retombe sur obrain
  (CBM_BRIDGE_BIN/OBRAIN_STORE). Rollback : restaurer le .off.
- À retester en UI : ouverture d'une conversation (messages).

## 18. CAUSE RACINE messages UI — trouvée (sous-agent, fichier:ligne) + correctif appliqué
- Chemin UI : doOpen → RPC history({maxMessages:50}) → session-persistence-graph.readRecent()
  → obrain bridge session_load en mode events_only (JOURNAL) → events:[] → écran vide SANS erreur.
- full-v2/runtime-full = stores NODES-ONLY (1 zone journal / 243 sessions) ; runtime-dev =
  330 zones journal / 131 sessions (les messages UI viennent des ZONES JOURNAL, pas des nœuds).
- Correctif appliqué (2026-09-04) : prod OBRAIN_STORE → runtime-dev.obrain (backup drop-in créé).
- VRAIE réparation full : matérialiser les zones journal dans le store full via import_events
  depuis les archives (source_path ~/.dsh/sessions, ~/.dsh-dev/sessions) — chantier d'écriture.
- Piste code (release) : fallback readRecent → loadByNodes quand page journal vide
  (packages/session/session-persistence-graph/src/index.ts:634).
- À valider : UI prod :3082 (ouverture conversation = historique affiché, pagination, envoi).

## 19. Audit câblage harness→obrain (sous-agent, 2026-09-04) — synthèse
- M1 lectures sessions : DÉJÀ obrain (99). M3 injection graph/context_build : obrain ✓.
- M2 écritures : prod RO (pas OBRAIN_WRITE) → import_events inopérant ; correctif = OBRAIN_WRITE=1
  sur STORE DÉDIÉ (jamais runtime-dev partagé).
- M4 brain_search : dernier canal CBM (défaut 'codebase-memory-mcp' par PATH) → code env-fallback
  (graph-injector.ts:78) ; à terme = graphe de code en obrain (chantier).
- M5 weave/engrams : outils MOTEUR obrain, pas de câblage harness requis.
- M6 row MCP codebase-memory (profiles/web/cordis.patch.yml) pointe binaire CBM (prod/dev) → cible
  obrain bridge ; résidus systemd cbm (cbm-runtime-sync actif) à éteindre après validation.
- M7 bundle prod = 01/09 ≠ worktree HEAD 28f08440e4 (persistence-graph : résilience bridge mort +
  pagination before_seq manquantes) → aligner via dsh-deploy.
- Correctif perf immédiat : OBRAIN_IDLE_MS neutralisé par UnsetEnvironment de 30-align-dev → drop-in
  '100-obrain-idle.conf' ajouté (ordre > 99).

## 20. Correctifs appliqués post-audit (2026-09-04)
- OBRAIN_IDLE_MS rétabli (drop-in 45-obrain-idle.conf, ordre > 30-align → env mergé = 15000).
- Prod :3082 = runtime-dev (journaux, messages UI OK attendus), RO. En attente :
  validation UI messages, puis M7 (dsh-deploy align bundle HEAD), M6 (row MCP → obrain),
  M2 (OBRAIN_WRITE sur store dédié), M4 (brain_search env-fallback) — chacun avec backup.

## 21. Persistance des correctifs M4/ops (session prod, 2026-09-04 après-midi)
Principe acté : tout correctif doit être PERSISTANT (source git) et REDEPLOYABLE
(le redeploy reproduit l'état actif) — jamais de patch à la main sur l'installé.

Commits posés :
- `harness-worktrees/graph-writer` feat/graph-writer `d31fbd4aae` : M4 —
  graph-injector.ts:78 env-fallback CBM_BRIDGE_BIN avant 'codebase-memory-mcp'
  (brain_search tombait sur le canal cbm par défaut PATH).
- `harness-worktrees/graph-writer` feat/graph-writer `05693a1e68` : drop-ins
  systemd source = état actif 04/09 (99-obrain-harness : runtime-dev + idle
  15000 + historique ; + 45-obrain-idle.conf nouveau). Avant : la source datait
  du 01/09 (runtime.obrain, idle 60000) → un redeploy aurait fait REVENIR la
  prod à l'ancien store.
- `projects/obrain` master `f88e906` : scripts/obrain-harness-bascule.conf
  aligné sur l'actif 04/09 (source du drop-in posé par bascule-obrain-harness.sh).

État : M4 non encore buildé ni déployé (build officiel = worktree entier avec
WIP dev 31 fichiers → appartient à la dev, fenêtre annoncée). Étapes lourdes
restantes inchangées : M7 dsh-deploy (embarque M4+HEAD), M6, M2 — ordre
recommandé (a)→(b)→(c) après validation dev.

## 21. Alignement ciblé du bundle prod (2026-09-04, sans deploy lourd)
- persistence-graph lib ← worktree 09-03 (résilience bridge-mort + pagination before_seq) ;
  backup horodaté. Patch M4 appliqué (3 constructeurs GraphInjector : fallback
  process.env.CBM_BRIDGE_BIN ?? 'codebase-memory-mcp') — backups .bak-m4-*.
- Prod :3082 redémarré, actif, HTTP 200. Store = runtime-dev (journaux).
- À valider UI : messages + « Charger plus tôt » (pagination), envoi, sélection projet,
  puis brain_search (fallback env) dans une nouvelle session.

## 22. Validation chemin UI (2026-09-04, store runtime-dev)
- session_load (params UI : events_only+exclude_chunks) c254ac35 → events=500, msgs=384 ;
  sessions_list 43 ms. Le chemin journal de l'UI répond.
- Prochaine : M6 = non un swap naïf de la row MCP (les outils mcp__codebase-memory__*
  sont cbm-spécifiques) mais le chantier « index de code sur obrain » ; M2 écriture sur
  store dédié après validation UI.

## 23. EPIPE « history unavailable » — correctifs (2026-09-04)
- Cause : bridge fermé (idle 15 s) pendant la requête → write EPIPE dans le process mort ;
  respawn/EPIPE pas rattrapé.
- Correctifs : session-persistence lib synchronisé (worktree 09-03, M4 réappliqué) ;
  OBRAIN_IDLE_MS → 600 000 (99 + 45) — store RO partagé, aucun conflit d'écriture.
- Prod :3082 redémarré, env idle 600 000. À retester : ouverture de session 735915bf….

## 23. Bug sélecteur de projet — diagnostic (sous-agent, fichier:ligne)
- Pick = ESPACE DE TRAVAIL : RPC workspace.list → WorkspaceRegistry.list (workspace.json,
  $DSH_HOME/storages) + sessions via bridge obrain (sessions_list). PAS le MCP codebase-memory.
- Cause : baseline workspace.list rejouée seulement à chaque connexion (manager.ts:267-269) ;
  échec initial (bridge froid au boot) → phase 'pending' figée → menu vide.
- Sondes prod actuelles : workspace.list OK (4 espaces), session.list 78, création OK côté serveur.
- Correctifs : (1) reload onglet (zéro config) ; (2) si persiste : retry/backoff dans
  workspaces/manager.refresh() + préchauffage bridge au boot ; (3) row MCP prod → binaire
  ingest/obrain (canal agent mort, pas le picker). Bundle prod = snapshot antérieur aux
  fixes dev 03/09 (reuse/groupage blanks).

## 24. Store DÉDIÉ PROD + écriture (2026-09-04)
- runtime-prod.obrain = clone runtime-dev (400 zones, 350 journaux). Prod : OBRAIN_STORE=
  runtime-prod, OBRAIN_WRITE=1, idle 300000 (chaud 5 min) → plus de contention avec le dev
  (runtime-dev). Rollback : env runtime-dev + restart.
- Objectif : « + » crée/attribue la conversation, menu se ferme, envoi OK, réactivité warm.

## 25. Patchs UI appliqués à la prod (2026-09-04)
- ui-workspace lib ← rebuild (source : onCreate → expandSidebar « + » ; openSession wrapper) ;
- runtime lib ← rebuild (écho optimiste noteSessionMembership à la création → auto-sélection) ;
- backups horodatés ; prod restarté. À valider : menu fermé sur « + » et conversation,
  auto-sélection projet fiable sur la page nouvelle conversation.

## 26. Empreinte mémoire obrain vs kuzu (2026-09-04, même corpus 445 k/1,67 M)
- RSS max mesuré (process entier, données touchées) : obrain ~257 Mo (dont Node ~50-70 →
  moteur ≈190-210) vs kuzu ~160 Mo (dont python ~20 → moteur ≈140). Disque : obrain 420 Mo
  (store trié + sidecars riches) vs kuzu 46 Mo (columnar compressé).
- Lecture : kuzu plus léger ici ; obrain porte des sur-structures fonctionnelles absentes chez
  kuzu (communautés, Z-order/fenêtres 2D, degrés typés, engrams, retrieval hybride). Avantages
  obrain en taille maintenus vs cbm (×6 disque) et vs moteurs LDBC testés (README) ; vs kuzu
  l'argument est la profondeur, pas l'empreinte. À affiner : bench RSS à pages touchées
  identiques.
