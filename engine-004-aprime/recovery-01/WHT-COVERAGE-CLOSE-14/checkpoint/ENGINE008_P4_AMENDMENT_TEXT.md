# AMENDEMENT P4 -- TEXTE EXACT, PRET A ARBITRER

**Statut : `PROPOSED_NOT_ADOPTED`.** Le protocole v2 reste
`822db99f6c288e346e1dca06c7405ab13a4a06d69bc05beccbe92e333fe5fa3d`.
Ce texte est pret a etre **adopte tel quel, amende, ou rejete**. Il ne modifie
rien par lui-meme.

---

## Texte de P4 actuelle (v2, inchangee)

> **P4 — propriete negative de rotation baseline.**
> Preuve statique : non. Observation future : WHT sur la baseline.
> Couverture necessaire : fenetre de capture valide, ledger VALID.
> Verdict autorise si : rotation absente AVEC couverture non nulle.
> INCOMPLETE / NOT_CONFIRMED si : couverture nulle.

**Non evaluable, etabli :** le collecteur f1a filtre sur
`w->type == GGML_TYPE_TQ2_G128` (`f1a-capture.cpp:442`). La baseline est
quantifiee **Q4_K_M** : aucune cible eligible, donc **couverture nulle**, et une
couverture nulle reste `NOT_CONFIRMED`. **Ajouter des devices ne change pas son
type de poids.**

---

## Texte propose pour P4'

> **P4' — inatteignabilite du chemin WHT pour la baseline.**
> **Propriete exigee :** aucun chemin de generation de WHT n'est atteignable
> pour les poids de la baseline.
> **Methode :** sur le modele reellement charge, inventorier les **types
> effectifs** des tenseurs atteignant `build_lora_mm`, et etablir qu'aucun n'est
> `GGML_TYPE_TQ2_G128` ; la branche
> `if (w->type == GGML_TYPE_TQ2_G128 && getenv("OBRAIN_NO_ROTATION") == nullptr)`
> (`llama-graph.cpp:1497`) est alors **inatteignable**.
> **Verdict autorise si :** les six conditions ci-dessous sont verifices.
> **INCOMPLETE / NOT_CONFIRMED si :** une seule manque.

### Les six conditions

1. **Acquisition complete du registre natif** du modele charge, via
   `llama_internal_get_tensor_map` -- pas un echantillon.
2. **Inventaire des TYPES REELS**, lus sur les tenseurs. **Interdit** de deduire
   le type du nom `Q4_K_M` ou d'une metadonnee de fichier.
3. **Aucun poids pertinent n'est `TQ2_G128`.** « Pertinent » = atteignant
   `build_lora_mm`, decrit dans la source comme le **goulot unique de toutes les
   projections lineaires de tous les modeles**.
4. **Audit des autres sites de creation WHT.** Etat actuel, par lecture :
   dans `src/`, `ggml_wht` n'est appele qu'en `llama-graph.cpp` ;
   `src/obrain-rotation.cpp` est l'outil **hors ligne** de quantification ;
   `ggml/src/ggml.c:3011` est la **definition** de l'op. A refaire sur un fork
   qualifie au moment du run.
5. **Identite du code applicable et conditions du chemin** : SHA de libllama,
   `OBRAIN_NO_ROTATION`, `ctx_type`, `gtype`.
6. **Limites de la conclusion**, telles qu'enoncees ci-dessous.

### Changement de propriete -- A DIRE EXPLICITEMENT A L'ARBITRAGE

**P4' ne remplace pas une propriete par une autre methode : elle remplace une
propriete par une AUTRE PROPRIETE.**

| | P4 (v2) | P4' (proposee) |
|---|---|---|
| Ce qui est etabli | une **ABSENCE OBSERVEE** | une **INATTEIGNABILITE STRUCTURELLE** |
| Sur quoi | une couverture d'observation | la construction du graphe |
| Portee | l'instrument f1a | le contrat du builder |

### Limites exactes de P4'

- elle **ne dit rien de l'execution** ;
- elle est **subordonnee a la condition 2** : un seul poids pertinent en
  `TQ2_G128` la fait tomber ;
- elle **ne remplace pas** P3 (rotation candidate), etablie autrement ;
- elle porte sur la **construction du graphe** ; le maillon
  construction -> graphe soumis -> logits reste `PARTIAL` (voir A3).

### Etat des conditions

**AUCUNE n'est verifiee sur un modele reel.**
`NATIVE_MODEL_IDENTITY_ACQUISITION=NOT_EXERCISED`, `FULL_MODEL_LOAD_COUNT=0`.
Les conditions 1, 2, 3 et 5 sont donc **NON VERIFIEES** ; la 4 n'a ete auditee
que par lecture de sources.

**L'amendement ne peut pas etre adopte aujourd'hui.** L'adopter reviendrait a
accepter une methode dont les conditions n'ont pas ete etablies.
