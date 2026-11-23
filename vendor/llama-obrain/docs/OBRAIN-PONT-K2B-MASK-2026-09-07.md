# Proposition — K2b : masque d'attention custom multi-têtes sur les positions mémoire

**Date** : 2026-09-07 — goal e9c190dd — fork llama-obrain. **Statut** : PROPOSITION (design + DoD).

## Pourquoi
L'injection par état (K2) place le KV d'un fait en tête d'une seq réservée ; le masque causal
standard rend ces positions visibles depuis le prompt (mesuré, fonctionne). K2b ajoute le **contrôle
fin** exigé au goal (b) : quelles **têtes** de quelles **couches** peuvent lire quelles **positions
mémoire**, via le mécanisme déjà présent (`custom_attn_mask`, `custom_attn_mask_n_head_groups`,
positions/triés, mask hiérarchique — llama-graph.h ; câblé au graphe llama-graph.cpp:496 et au
contexte llama-context.cpp:2902-2906).

## Design
1. **Modèle de masque** : matrice par (couche optionnelle) position_courante × position_antérieure,
   avec granularité **par groupe de têtes** (`n_head_groups`) — ex. le fait mémoire n'est lu que par
   un sous-ensemble de têtes (les « têtes de faits »), le reste du modèle reste sur le contexte
   visible ; contrôle aussi des positions « mémoire réservées » vs positions prompt.
2. **Entrée** : dans la route d'injection par ref, champs optionnels :
   `{state_ref, prompt, attn: {head_groups: n, allow_mem_pos: bool|"heads":[…]}}` → sérialisés vers
   les accesseurs existants du contexte (`set_attn_mask*`) avant le decode du prompt ; défaut =
   masque causal actuel (aucun changement de comportement si absent).
3. **Multi-états** : positions mémoire réservées contiguës par état ; le masque peut n'exposer qu'un
   sous-ensemble d'états à un instant (contrôle par requête) — premier pas vers la « mémoire par
   couches/têtes ».
4. **Serveur** : réutilise le plumbing custom_attn_mask déjà câblé (aucun nouveau champ graphe) ;
   seul le parsing route + réglage pré-decode est à ajouter dans le task JSPACE_INJECT_KNOWLEDGE.

## DoD (8 cases)
1. Route accepte `attn.{head_groups, allow_mem_pos}` ; valeurs transmises aux accesseurs contexte ;
   défaut identique au comportement actuel (diff ∅ sans le champ).
2. Test de contrôle : avec masque restrictif (têtes « faits » seules), la réponse change vs masque
   plein sur un fait ciblé (mesure qualitative + bench).
3. Vérification multi-têtes : `n_head_groups` accepté et appliqué (pas d'erreur, shape cohérente).
4. Multi-états : 2 refs injectés sur positions distinctes, masque n'exposant qu'un des deux → seule
   la connaissance exposée influence (test).
5. Bench daté : inject+mask vs inject simple (CPU puis GPU) — overhead masque.
6. Aucune régression (build fork vert ; défaut inchangé) ; commits réversibles.
7. Docs (ce fichier + README adapter) + graph.
8. GPU pour les chiffres temps réel (CPU valide la mécanique).
