# ENGINE008_TEMPLATE_AUTHORITY_DECISION_v1

**Passe :** PROTOCOL-HARNESS-01 — 2026-10-02
**Statut :** `CHAT_TEMPLATE_AMENDMENT_STATUS = ADOPTED_COMMON_BASELINE`

---

## 1. Décision adoptée

L'arbitrage scientifique a été **adopté** :

- **Objectif :** comparer baseline et candidat avec des entrées **contrôlées,
  bit-identiques**.
- **Autorité de template :** les octets exacts du template **baseline**.
- **Renderer :** le **véritable moteur Jinja natif** du fork Q1
  (`common_chat_templates_apply`).
- **Les templates embarqués propres aux deux GGUF ne sont pas utilisés
  individuellement** pour produire les entrées de cette évaluation.
- **Leur divergence reste documentée.**
- **Aucun GGUF n'est modifié.**

---

## 2. Les deux empreintes embarquées — conservées et déclarées

| Modèle | `tokenizer.chat_template` SHA256 | Octets |
|---|---|---|
| Baseline | `c3cf9e34abf4f9e36c2d72165aa9c132d3e2a725b6c2586aaa3a8af9d7a81041` | 8 952 |
| Candidat | `12827f24b742ea4e80cdc12dbcf9622227056b9f797252a3149263d4f9aaadce` | 9 993 |

```text
EMBEDDED_CHAT_TEMPLATE_MATCH=NO
```

Première divergence à l'octet 1971 ; 102 lignes de diff unifié (+34 / −20).
Le candidat se termine par `{#- Unsloth fixes - developer role, merged system
messages, tool calling #}`.

**Cette divergence n'est pas résolue, ni atténuée, ni renommée.** Aucun GGUF n'a
été retouché pour obtenir artificiellement l'égalité des empreintes.

---

## 3. Autorité effective commune

```text
ENGINE008_EFFECTIVE_CHAT_TEMPLATE_SHA256 =
  c3cf9e34abf4f9e36c2d72165aa9c132d3e2a725b6c2586aaa3a8af9d7a81041
```

Artefact : `ENGINE008_EFFECTIVE_CHAT_TEMPLATE.jinja`, 8 952 octets.
Provenance : octets exacts du template embarqué du modèle **baseline**, extraits
de `tokenizer.chat_template`, transmis **explicitement** aux deux chemins.

Le choix de la source « baseline » a été énoncé **avant tout rendu et avant tout
score** : c'est le bras de référence de la comparaison, ce qui rend l'écart
ultérieur imputable au candidat.

---

## 4. Distinction maintenue entre les deux notions d'égalité

```text
EMBEDDED_TEMPLATE_MATCH   = NO             (mesuré ; ne changera pas)
EFFECTIVE_TEMPLATE_MATCH  = YES_MEASURED   (exercé et mesuré, voir §5)
```

La gate historique **n'est pas rebaptisée rétroactivement** pour la faire passer :
les deux valeurs sont publiées côte à côte.

---

## 5. Exercice de l'égalité effective — mesuré

Sur les **2 698 items** TASK, rendus via le chemin natif avec l'autorité commune :

```text
EFFECTIVE_TEMPLATE_BASELINE_CANDIDATE_MATCH = YES
TASK_RENDERED_TEXT_BASELINE_CANDIDATE_MISMATCH_COUNT = 0
TASK_INPUT_TOKEN_ID_BASELINE_CANDIDATE_MISMATCH_COUNT = 0
```

Preuve que le candidat **ne récupère pas** son template embarqué par défaut
(mesuré par le harnais, mode `renderer-qualify`) :

```text
MODEL_EMBEDDED_TEMPLATE_SHA256            = 12827f24b742ea4e80cdc12dbcf9622227056b9f797252a3149263d4f9aaadce
RENDERER_TEMPLATE_SOURCE_SHA256           = c3cf9e34abf4f9e36c2d72165aa9c132d3e2a725b6c2586aaa3a8af9d7a81041
EMBEDDED_TEMPLATE_OVERRIDDEN_BY_EXPLICIT  = YES
```

Les six fixtures de qualification rendent des SHA256 **identiques** pour les deux
modèles, ce qui confirme que le rendu est indépendant du modèle.

---

## 6. Conséquences assumées, exposées sans détour

Le candidat est évalué sur un texte qu'il **n'a pas embarqué**. La comparaison
mesure donc les **poids**, pas la fidélité au template livré. C'est le coût
explicite de l'objectif `CONTROLLED_IDENTICAL_INPUTS`, et il a été accepté
**avant** toute observation de score.

L'objectif alternatif (fidélité native : chaque modèle rendu avec son propre
template) reste **incompatible** avec l'égalité §20 des entrées. Il n'a pas été
retenu. Les deux ne peuvent pas coexister.

---

## 7. Ce que cette décision n'autorise pas

- Aucun rendu au titre d'une autre autorité de template.
- Aucune modification des GGUF, de leurs commentaires ou de leurs templates.
- Aucun calcul modèle, même après le gel obtenu dans cette passe.
- Aucune reconstitution de l'un des templates à partir de l'autre.
