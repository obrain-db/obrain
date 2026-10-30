# ENGINE008_WHT_EXECUTION_CONTRACT — preuve d'execution

## 1. `submission_rc` n'est PAS un statut d'execution

`llama-context.cpp:2952-2960`, verbatim :
```cpp
// [F1A-CAPTURE] R2 : HOOK AUTORITAIRE DE SOUMISSION DE CALCUL.
// C est ICI, et seulement ici, qu un graphe est prouve SOUMIS.
auto status = ggml_backend_sched_graph_compute_async(sched.get(), gf);
// [F1A-CAPTURE] R3 : instantane FIGE du graphe soumis, pris immediatement apres
// l appel, avec son resultat. C est la seule preuve de soumission.
if (f1a_capture_get_ctx() != nullptr) f1a_mark_compute_submission(gf, (int) status);
```

`submission_rc` recoit le retour de **`..._graph_compute_ASYNC`**, et
`GGML_STATUS_SUCCESS = 0` (`ggml/include/ggml.h:361`). Le producteur qualifie
lui-meme ce point : « la seule preuve de **SOUMISSION** ».

**Donc `submission_rc=0` ne prouve pas l'execution** -- et la consigne a raison de
l'interdire. C'est une acceptation, pas un achevement.

## 2. Le maillon d'achevement existe et est localise

`llama_context::synchronize()` (`llama-context.cpp:718-723`) appelle
`ggml_backend_sched_synchronize(sched.get())` ; il est invoque sur le chemin de
decode (`:1954`, `:2620`). C'est ce qui transforme une soumission acceptee en
calcul **acheve**.

## 3. Le scheduler parcourt TOUT le graphe

`ggml-backend.cpp` itere `for (int i = 0; i < graph->n_nodes; i++)` en de nombreux
points (`:947, 1045, 1080, 1122, 1160, 1214`). Un noeud present dans le graphe
soumis est donc traite par le scheduler.

## 4. La methode, et sa portee exacte

**Il n'existe PAS de trace d'operation par operation.** Les surfaces f1a
observent la CONSTRUCTION et la SOUMISSION, jamais le deroule du backend.
`effect_ok`, `role_ok`, `in_submitted_graph`, `resident` ne sont pas des traces
d'execution, malgre leurs noms.

**Methode retenue : DEDUCTION JUSTIFIEE PAR LE CONTRAT DE CALCUL.**

Chaine :
1. le graphe `gf` est soumis ; `submission_rc == GGML_STATUS_SUCCESS` (accepte) ;
2. `synchronize()` acheve le calcul avant que les sorties soient lues ;
3. le scheduler traite tous les `n_nodes` du graphe soumis ;
4. la WHT est un noeud de ce graphe (`in_submitted_graph=1`, `sub_at_build=1`) et
   son resultat atteint la sortie (`downstream_reach=1`,
   `effect_class=OUTPUT_DEPENDENT`, `role_ok=1`, `effect_ok=1`) ;
=> **sous ces conditions, l'operation WHT a ete executee** comme partie du graphe
soumis et acheve.

**Hypotheses, explicitees :**
- **A1** le scheduler execute tous les noeuds du graphe soumis -- *verifie par
  lecture* (`ggml-backend.cpp`, iteration sur `n_nodes`).
- **A2** la WHT est un noeud du graphe soumis -- *verifie sur pieces*
  (`in_submitted_graph=1` sur 504/504 lignes de residence).
- **A3** les noeuds d'un graphe ggml sont la cloture transitive des sorties
  demandees (`ggml_build_forward_expand`), donc etre un noeud = etre
  output-dependant. **NON verifie dans cette passe** ; corrobore par
  `downstream_reach=1` et `effect_class=OUTPUT_DEPENDENT` du producteur.
- **A4** `synchronize()` acheve avant la lecture des logits -- *localise* sur le
  chemin de decode ; l'ordre exact vis-a-vis de `llama_get_logits_ith` du harnais
  reste a confirmer au runtime.

## 5. Ce que cette methode N'etablit PAS

- **Elle ne dit rien par pas de decode.** Le cache `obrain_rot` partage une
  activation entre consommateurs : `rotation_applied=1` marque que le graphe
  *contient* la transformee pour cette activation. Si le cache l'a servie, le
  noeud est partage. **CREATION != EXECUTION INITIALE != CONSOMMATION
  ULTERIEURE.** Une nouvelle execution WHT a *chaque pas* n'est PAS etablie ;
  « la WHT a ete executee au moins une fois dans un graphe soumis et acheve »
  l'est.
- **Elle n'est pas une preuve directe.** C'est une deduction dont les hypotheses
  sont nommees et dont A3 reste a etayer.
- **Elle ne remplace pas** la couverture : si le graphe n'est pas correle a la
  soumission, ou si la surface manque, la deduction tombe.

## 6. P3 : admissibilite sous le texte actuel de v2

Texte exact de v2 : *« P3 | Rotation candidate | preuve statique : non |
observation future : WHT sur le candidat | couverture necessaire : fenetre de
capture valide, ledger VALID | verdict autorise si : rotation detectee,
couverture suffisante | INCOMPLETE si : ledger ABSENT/EMPTY/PARTIAL/MALFORME »*.

**P3 n'exige PAS de trace d'execution par operation.** Il exige
« rotation detectee, couverture suffisante ». La methode etablie fournit :
- rotation detectee : `rotation_applied=1` sur les cibles ;
- couverture : cibles distinctes, valeurs illisibles comptees, `kind` distingue ;
- et, en plus, la consommation dans le graphe soumis.

**P3_CURRENT_PROTOCOL_ASSESSABLE=YES**, par une methode qui SATISFAIT l'exigence
au sens litteral. La deduction d'execution (§4) est un **renfort**, pas une
condition de P3 -- et elle est documentee comme deduction, avec ses hypotheses.

**Aucun amendement de P3 n'est donc necessaire.** Si l'arbitrage veut exiger la
trace par operation, ce serait une exigence PLUS FORTE que v2 et exigerait un
amendement explicite -- que je ne propose pas ici.
