# Reconstruction de l'agent — protocole de reprise (limites de contexte)

**Date** : 2026-09-05 — branche `feat/zorder` — **À LIRE EN PREMIER après toute
perte de contexte / redémarrage / compaction.**

But : reconstruire un état exploitable avec un budget de contexte borné et un
ordre déterministe (PULL — je ne me souviens pas, je relis des artefacts
durables). Ce fichier est la couche L0 de ma propre mémoire.

---

## 1. Cas réels vécus (ce protocole répond à ces échecs)

| # | Événement (2026-09-05) | Cause | Leçon |
|---|---|---|---|
| 1 | Coupure dure de session à 14:40 (erreur 400, limite) en plein diagnostic | contexte épuisé | le point de reprise doit être un ARTEFACT (commit/ledger), pas ma mémoire |
| 2 | Reprise froide : j'ai dû tout reconstruire (journaux binaires, goals, ledger) | contexte zéro | il faut une ENTRÉE unique + des commandes exactes |
| 3 | Continuation parallèle : 48 commits invisibles pour moi (doc `ETAT` créée pour ça) | sessions concurrentes sur le même repo | le VRAI état = git ; mes vues mentales sont suspectes → détecter l'écart |
| 4 | Sujet BORNÉ : l'utilisateur parle d'un sujet précis (feature, bug, idée) SANS besoin du contexte global du projet | ma tentation = tout relire (repo, ledger, HEAD) | 2 ROUTES : **globale** (repo) OU **sujet** (ne lire QUE la tranche du graphe liée au sujet) — décider AVANT de lire |

Limites assumées : contexte fini (pas de « tout retenir »), mémoire = fichiers
durables + graphe (obrain/brain), récupération = lecture ciblée uniquement.

## 1bis. DÉCIDER LA ROUTE AVANT DE LIRE (cas n°4)

Question : **le sujet demandé a-t-il besoin du contexte global du projet ?**
- **NON (sujet borné)** — ex. « parle-moi de search_conflicts », « cette feature X »,
  « cette idée » : route SUJET. Ne PAS lire repo/ledger/HEAD.
  1. `brain_search "<sujet>" (cognitive puis exact)` → nœuds/sessions qui en parlent ;
  2. coder la « tranche » : les nodes du sujet (Feature/Module/Doc/Commit) + leurs
     voisins à 1-2 sauts (qui le cite, quelle décision, quel bug lié) ;
  3. `runtime_search "<sujet>"` / sessions du sujet (sessions_list filtrées) ;
  4. ne lire un fichier que s'il apparaît dans la tranche.
  Budget : ~2-4 k tokens, pas d'état global requis.
- **OUI (travail sur le repo)** : route GLOBALE = L0 ci-dessous.
- **Compétence demandée** (« sais-tu faire X ? ») : route COMPÉTENCE → chercher le
  nœud `Skill` du graphe (§7) au lieu de scanner les docs.

## 2. Ordre de reconstruction (coûts cibles, budget ≈ 8 k tokens pour L0)

### L0 — colonne vertébrale (~4-6 k tokens)
1. `git log --oneline -25 && git status --short` → HEAD réel, propreté.
2. Lire **ce fichier** (L0) puis `docs/ETAT-2026-09-05-2015.md` (état consolidé)
   puis `docs/CAPACITES-AGENT-OBRAIN-2026-09-05.md` (ce que je sais faire)
   puis **`docs/PLAN-TISSAGE-PERIMETRE-2026-09-06.md`** (carte du périmètre
   courant : multi-writer + contrat bridge — la tenir à jour).
3. `tail -6 docs/ledger-ameliorations-2026-09-04.md` → dernier chantier consigné.
4. **Objectif courant** : `get_goal` (in-session) ; s'il est absent, le dernier
   goal dans le graphe runtime (`brain_search "goal" exact`) et le dernier
   message utilisateur du journal (cf. §4).
5. **Détection d'écart (drift)** : comparer le HEAD annoncé dans les docs L0 au
   HEAD git → si différent, lire `git log --oneline <head-annoncé>..HEAD` (le
   delta EST le « search_delta » de mon état) ; si worktree non propre →
   `git diff --stat` (travail en cours).

### L1 — chantier courant / en cours (à la demande)
- `ls -t docs/PROPOSAL-* docs/CHECKPOINT-* docs/ETAT-* 2>/dev/null | head`
  → propositions & ré-ancres récentes ; lire la plus récente pertinente.
- Sujets : `git log --oneline -S '<motif>' -5` (ex. search_surprise, relief).
- Tests du domaine : `ls crates/obrain-engine/tests/search_*.rs` +
  `cargo test -p obrain-engine --features cdc,algos --test <cible>`.
- Travail non commité = LA priorité avant toute nouvelle action (risque de
  perte) : `git diff --stat` + décider commit immédiat.

### L2 — détails (jamais par défaut)
- Code précis, benchs, rangées ledger complètes, graphe (brain_search
  exact/cognitive), sessions précédentes (cf. §4).

## 3. Récupérer une conversation / session antérieure

La DSH capture tout dans obrain :
- **Graphe runtime** (`dsh-runtime`) : `sessions_list` → session cible ;
  `session_load` ; `brain_search "<terme>" (exact|cognitive)` pour retrouver
  un sujet ; `context_build` pour l'injection.
- **Journaux bruts** (`~/.local/share/obrain-harness/data/runtime-dev.obrain/
  substrate.journal.<session>`) : format zones `OBJ1` (header 20 B : magic,
  count u64, used u64 ; puis frames `[len u32][bincode string JSON]` —
  décoder via le script inline python ci-dessous, jamais de logique livrée).
- **Moi-même comme graphe** : `search_delta`/`search_surprise`/`search_conflicts`
  sur le store de session = diff/saillance/contradictions de MA propre trace.

```
# Décodeur minimal du journal de session (frames bincode → JSON).
# Usage : python3 dec.py <fichier> | head (remplacer par vos besoins).
python3 - <<'EOF'
import json, struct, sys
b=open(sys.argv[1],'rb').read()
used=struct.unpack('<Q',b[12:20])[0]; off=20; end=min(used,len(b))
out=[]
while off+4<=end:
    n=struct.unpack('<I',b[off:off+4])[0]; off+=4
    if not n or off+n>end: break
    fr=b[off:off+n]; off+=n
    for p in (0,1,2,4,8):
        s=fr[p:]
        if s[:1]==b'{':
            try: out.append(json.loads(s)); break
            except Exception: pass
print('\n'.join(json.dumps(e,ensure_ascii=False) for e in out))
EOF
```

## 4. Fermer la boucle : le « moment zéro » d'une reprise

1. L0 complet (ordre ci-dessus) → 2. Écart : HEAD vs docs, worktree vs commit →
   3. Objectif : goal ou dernier message utilisateur → 4. **Je reformule par
   écrit en 3 lignes** « état réel / travail en cours / prochaine action » et je
   la consigne (ledger row ou doc) — c'est CETTE phrase qui survivra à la
   prochaine coupure → 5. J'agis sur un seul fil.

## 5. Ce que les formes de recherche m'apportent ici (non pas décor, mais outil)

- `search_delta(since)` = diff de mon état (je l'utilise pour le drift HEAD) ;
- `search_surprise(since, k)` = ce qui a changé CONTRE l'attente (reprendre
  par la saillance, pas par l'index) ;
- `search_conflicts(since, k)` = mes docs qui se contredisent (deux ré-ancres
  avec des HEAD différents) ;
- `search_causes/effects(id)` = remonter la chaîne de décisions qui a mené à
  l'état courant ;
- `search_by_role(id)` = trouver dans une AUTRE variante/session le « même
  rôle » que ce que je cherche (reprendre un pattern connu ailleurs).

## 6. Vérification (6 cases, coût ~1-2 k tokens)

- [ ] `git log/status` lu → HEAD + propreté connus ;
- [ ] L0 lu (ce fichier + ETAT + CAPACITES) ;
- [ ] dernier chantier ledger identifié ;
- [ ] écart HEAD docs↔git mesuré et expliqué (ou nul) ;
- [ ] objectif courant identifié (goal ou dernier message) ;
- [ ] travail non commité traité (commité ou explicitement différé).

## 7. Compétences dans le graphe (convention — structuration, pas docs)

Chaque compétence développée devient un nœud `Skill` interrogeable (au lieu
d'uniquement un doc) :
- qn : `skill:<nom>` — props : `description`, `surface` (ex. `CALL obrain.x`),
  `confiance` (testé/synthétique), `date`, `commit`.
- Relations structurées entre compétences et avec le code :
  `BUILDS_ON` (prérequis), `RELATES_TO` (feature/commit), `EVIDENCE_BY`
  (test/bench), `SUPERSEDES`.
- Lecture à la reprise : `brain_search "skill" exact` / cognitive sur le sujet →
  la tranche des compétences pertinentes, sans tout recharger.
- Convention appliquée dès maintenant pour les compétences moteur (recherches,
  relief, compare, CDC fenêtres…).

## 8. TISSER PENDANT LA RÉSOLUTION — règle de méthode (2026-09-05)

Principe : **je ne peux relire que ce que j'ai écrit**. À la reprise, rien ne
survit hors des artefacts durables — donc le tissage n'est pas une tâche à part,
c'est une PHASE de chaque résolution (avant de la déclarer finie) :

1. **Écrire au fil de l'eau**, pas seulement à la fin : chaque décision/chiffre/
   correctif → rangée ledger datée + commit réversible (message = résumé
   re-lisible) ;
2. **Relier** le travail aux nœuds du graphe : compétence développée → nœud
   `Skill` (props surface/confiance/commit) + liens `BUILDS_ON/RELATES_TO/
   DOCUMENTED_BY` ; doc écrite → nœud `KnowledgeDoc` ABOUT son `Theme` ;
   manifest `docs/CONNAISSANCES-MANIFEST.json` = miroir re-seedable ;
3. **Vérifier la lisibilité future** : « un moi à contexte zéro retrouverait-il
   ce résultat ? » — si non, la résolution n'est pas finie (il manque le fil).

Les deux aspects : (a) l'ÉCRITURE/tissage pendant la résolution (ce qui rend la
connaissance durable et structurée) ; (b) la LECTURE ciblée ensuite (routes
sujet/compétence via le graphe). Le second est borné par le premier : on ne lit
que ce qui a été écrit.
