# AMENDEMENT P4 -- `PROPOSED_NOT_ADOPTED`

**Statut : PROPOSE, NON ADOPTE. Le protocole v2 reste INCHANGE**
(`822db99f6c288e346e1dca06c7405ab13a4a06d69bc05beccbe92e333fe5fa3d`).
Ce texte existe pour permettre l'arbitrage ; il ne modifie rien par lui-meme.

## P4 actuelle (v2)

> Absence de rotation observee sur la baseline, avec couverture native suffisante.

**Pourquoi elle n'est pas evaluable :** l'instrument f1a est filtre sur
`w->type == GGML_TYPE_TQ2_G128` (`f1a-capture.cpp:442`). La baseline est
quantifiee **Q4_K_M** : aucune cible eligible, donc **couverture nulle**, et une
couverture nulle reste `NOT_CONFIRMED`, jamais une absence. **Ajouter des devices
ne change pas son type de poids** -- le filtre est sur le type, pas sur la
topologie.

## P4 proposee

> **Inatteignabilite du chemin WHT identifie**, sous conditions verifiees sur le
> modele charge et le calcul qualifie.

Fondement : la seule branche qui insere `ggml_wht` est
```cpp
if (w->type == GGML_TYPE_TQ2_G128 && getenv("OBRAIN_NO_ROTATION") == nullptr)
```
(`llama-graph.cpp:1497`). Si **aucun** poids pertinent n'est `TQ2_G128`, cette
branche est **inatteignable** et la WHT **ne peut pas** y etre appliquee.

## Conditions EXIGEES avant que cette methode vaille

1. **Acquisition complete du registre natif** du modele reellement charge, via
   `llama_internal_get_tensor_map` -- pas un echantillon.
2. **Inventaire des TYPES REELS**, lu sur les tenseurs. **Interdit** de deduire le
   type du nom `Q4_K_M` ou d'une metadonnee de fichier : c'est le type effectif du
   tenseur qui gouverne la branche.
3. **Verification qu'aucun poids pertinent n'est `TQ2_G128`.** « Pertinent » =
   les poids atteignant `build_lora_mm`, soit, d'apres le commentaire source, le
   **goulot unique de toutes les projections lineaires de tous les modeles**
   (attention q/k/v/o, trois projections MLP, sortie, toutes architectures).
4. **Audit des autres sites de creation WHT.** Etat de cet audit : dans `src/`,
   `ggml_wht` n'est appele qu'en `llama-graph.cpp` (la branche ci-dessus) ;
   `src/obrain-rotation.cpp` est l'outil **hors ligne** de quantification et
   n'est pas sur le chemin d'inference ; `ggml/src/ggml.c:3011` est la
   **definition** de l'op. Cet audit doit etre refait sur un fork qualifie.
5. **Identite du code applicable et conditions du chemin** : SHA de libllama,
   valeur de `OBRAIN_NO_ROTATION`, `ctx_type`, `gtype`.
6. **Limites exactes de la conclusion**, enoncees ci-dessous.

## Limites EXACTES -- a conserver telles quelles

- Elle etablit une **INATTEIGNABILITE STRUCTURELLE**, **pas** une **ABSENCE
  OBSERVEE**. Ce sont deux proprietes differentes, et P4 actuelle demande la
  seconde. L'amendement **change donc la nature de la propriete**, pas seulement
  sa methode -- l'arbitrage doit le dire explicitement.
- Elle porte sur la **CONSTRUCTION DU GRAPHE**. Le maillon
  construction -> graphe SOUMIS -> execution reste, et **aucune surface auditee
  n'observe l'execution** (§3 de la cartographie).
- Elle est **subordonnee a la condition 2** : si un seul poids pertinent est
  `TQ2_G128`, elle tombe et la baseline redevient sans methode.
- Elle **ne remplace pas** non plus l'evaluation de la rotation candidate (P3),
  qui reste etablie par le manifeste.

## ETAT DES CONDITIONS : AUCUNE n'est verifiee sur un modele reel

**Aucune acquisition native n'a eu lieu** dans ce chantier
(`NATIVE_MODEL_IDENTITY_ACQUISITION=NOT_EXERCISED`,
`FULL_MODEL_LOAD_COUNT=0`). Les conditions 1, 2, 3, 5 sont donc **NON VERIFIEES**,
et la condition 4 n'a ete auditee que par lecture de sources, pas sur un fork
qualifie au moment du run.

L'amendement ne peut donc pas etre adopte aujourd'hui : l'adopter reviendrait a
accepter une methode dont les conditions n'ont pas ete etablies.
