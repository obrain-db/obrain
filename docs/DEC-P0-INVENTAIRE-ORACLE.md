# DEC-P0 — L'oracle : inventaire des points de décision + verdict (2026-10-03)

Porte d'entrée de l'ADR COUCHE-DECISION. Produit par 3 agents (lecture du code réel).

## Verdict : GO ✅

3 décisions à vérité observable **hors du modèle** (seuil ≥3 atteint) :

1. **Correction de bug** (n3__write / str_replace + n3__exec) — vérité = exit code, diff appliqué, test vert, relecture disque. Source : OS/git, pas le modèle.
2. **Délégation harness** (harness_delegate + task_status) — vérité = `turn/end.reason.kind == "completed"`. Source : machine à états dsh. (Jamais `dernier_texte` = sortie d'un sous-modèle.)
3. **Arrêt** (conclure sans tool_call) — vérité = relance humaine OU critère objectif atteint (test/build/diff). Limite : tâche ouverte → seule la relance humaine tranche.

**Risques DEC-X3 (écho, refusés)** : « l'agent dit corrigé » ≠ corrigé ; « dit pertinent » ≠ pertinent ; « a conclu » ≠ tâche finie ; `dernier_texte` du harness ; la confirmation humaine (c'est la décision de l'UTILISATEUR, pas de l'agent).

## Points de décision de l'AGENT (chat wt-chat-pieces-jointes)

| Décision | Qui décide | Forme | Lignes |
|---|---|---|---|
| Sélection d'outil | le modèle (borné par IAM + mode) | Choix multi-label | inference.rs:994-997, 40, 865 ; mod.rs:3748 |
| Condition d'arrêt | le modèle (finish_reason) | Binaire (Arret/Longueur/AppelsOutils) | inference.rs:777,857-864 ; mod.rs:3884-3886 |
| Objectif | le prompt (PAS le modèle) | cadrage imposé | mod.rs:225-239, 1044-1052, 3435 |
| Confirmation | jamais le modèle (nature statique + decider pur + utilisateur) | Binaire | outils/mod.rs:780-805 ; approbation.rs:183-198 |

## Points de décision du MOTEUR COGNITIF (7 modules, ~40 constantes)

- **search** : w_sim 0.3, w_energy 0.3, w_topo 0.3, w_synapse 0.1 (cognitive_procedures.rs:445-448) — ordinal.
- **intent** : 7 catégories, mots-clés, seuil pattern mort energy<0.05, window 300s (intent.rs) — choix multi-classes.
- **reflex** : confiance EMA, boost +0.1 / pénalité −0.15, dissolution energy≤0.01 (reflex.rs) — binaire.
- **immune** : affinity_radius 0.3, mutation 0.1, idiotypic 0.7, T-reg 0.3 (immune.rs) — binaire/score.
- **decay (FSRS-5)** : 19 poids, rétention R=(1+t/9S)⁻¹ (decay.rs) — score continu.
- **gap_detection** : AdamicAdar, top_k 5, min_link_score 0.15, twin 0.7 (gap_detection.rs) — binaire/ordinal.
- **risk (fabric)** : **somme pondérée de 6 termes normalisés** (w_pagerank 0.25, w_churn 0.25, w_annotation_gap 0.20, w_knowledge_gap 0.0, w_betweenness 0.15, w_scar 0.15), clampée [0,1] (fabric.rs:474-563) — ordinal.

## ⚠️ Correction à l'ADR (à porter)

L'ADR §1 et §16.2 affirment que `obrain.risk` = **produit** `churn × pagerank × gap × betweenness`. C'est FAUX : le code réel (`fabric.rs:556-563`) est une **somme pondérée normalisée de 6 termes**, avec le commentaire explicite « the weighted sum approach avoids the multiplicative collapse to zero » (`fabric.rs:531-532`). La docstring `cognitive_udfs.rs:72` perpétue la même erreur.

## Prochaine porte : DEC-P1 (le registre)

`(:Decision)` + `(:DecisionOption)` avec comptes `n`/`k`, postérieur Beta `p=(k+α)/(n+α+β)`, `acted` + `source` + `SUIT_DE` (DEC-C25). Les 3 oracles ci-dessus sont les `settle` possibles : exit code, `turn/end.reason.kind`, relance humaine.
