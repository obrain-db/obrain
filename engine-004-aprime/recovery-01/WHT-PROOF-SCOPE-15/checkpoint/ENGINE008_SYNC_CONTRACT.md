# ENGINE008_SYNC_CONTRACT — synchronisation et critere runtime

## La lecture des logits SYNCHRONISE -- etabli dans la source

`src/llama-context.cpp`, verbatim :

```cpp
float * llama_get_logits_ith(llama_context * ctx, int32_t i) {
    ctx->synchronize();          // <-- LA LECTURE FORCE LA SYNCHRONISATION
    float * res = nullptr;
    res = ctx->get_sampled_logits_ith(i);
    if (!res) res = ctx->get_logits_ith(i);
    return res;
}
```

Et `llama_context::synchronize()` (`:718-723`) appelle
`ggml_backend_sched_synchronize(sched.get())`.

**`LOGITS_SYNCHRONIZATION_CONTRACT_ESTABLISHED=YES`.**

| Question | Reponse |
|---|---|
| la lecture force-t-elle une synchronisation ? | **OUI**, `ctx->synchronize()` en premiere instruction |
| quel travail attend-elle ? | tout le travail soumis au scheduler, dont le graphe du pas courant |
| comment succes/echec est-il rapporte ? | par le **pointeur de retour** : `nullptr` si aucun logits n'est disponible, sinon un pointeur valide |
| ce que le harnais peut enregistrer sans modifier Q1 | soumission (`submission_rc`), retour de la lecture (`nullptr` ou non), et le contenu des logits lus |

**B4 EST FERME.** Le point d'observation existe deja et le harnais s'en sert :
`d.logits` appelle `llama_get_logits_ith` et rend `nullptr` si la lecture echoue
(le pipeline traite alors `StopReason::ERROR`).

## Critere du futur run

**Soumission -> achevement pertinent -> lecture reussie des logits**, materialise
ainsi :
1. `submission_rc == GGML_STATUS_SUCCESS` -- **evenement directement observe**
   (`llama-context.cpp:2955`, retour de
   `ggml_backend_sched_graph_compute_async`) ;
2. `llama_get_logits_ith` retourne un pointeur **non nul** -- **evenement
   directement observe**, et il **implique** la synchronisation par le contrat
   ci-dessus ;
3. la WHT est dans la cloture de la racine des logits -- **deduit de la source**
   (`qwen35.cpp:239-241`) et corrobore par l'inventaire des cibles (toutes des
   poids du tronc).

**DISTINCTION EXIGEE, tenue :**
- **evenements directement observes** : 1 et 2 ;
- **ordre deduit du contrat source** : que (2) acheve le travail soumis en (1),
  via `ctx->synchronize()` ;
- **jamais observe** : le deroule du backend operation par operation. Aucune
  surface ne le donne.

## Ce qui reste NOT_EXERCISED

`RUNTIME_OBSERVER_VALIDATION=NOT_EXERCISED`. La confirmation runtime consiste a
**enregistrer** ces deux evenements pendant les quatre runs, sans modifier Q1.
**Elle n'est PAS exigee comme resultat deja obtenu avant l'autorisation du
preflight** : c'est un critere du run, pas un prerequis.

## Methode precise

A chaque pas, le harnais enregistre : `submission_rc` du ledger, le retour non
nul de `llama_get_logits_ith`, l'`epoch` et le `decode_step` de l'extraction
correspondante, et le verdict WHT de la fenetre. Le critere est satisfait si,
pour **chaque** fenetre, on a : soumission acceptee **et** logits lus **et**
couverture WHT non partielle.
