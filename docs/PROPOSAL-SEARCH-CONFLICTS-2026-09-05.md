# Proposition — chantier `obrain.search_conflicts` (forme de recherche n°4)

**Date** : 2026-09-05 — branche `feat/zorder`, HEAD `2bfb03bd` — objectif « formes de
recherche » (goal 29a91d8c ; livrés : search_delta `c935b9d1`, search_surprise
`e921f98c`, search_by_role `2bfb03bd`).
**Statut** : v1 EXÉCUTÉE (round 6, commit du chantier) — tests 6/6, bench daté dans le ledger (fenêtre runtime : retour vide, aucune dissonance).

---

## 1. Réalité du code (reconnaissance) — pas d'ontologie « claims »

Aucun type « claim » ni moteur de contradiction textuelle n'existe dans obrain
(reconnaissance : dissonance/conflict/claim = conflits d'écriture, parser, etc.).
MAIS le CDC journal fournit exactement la matière d'une dissonance **d'état** :
`CdcLog::record_update(entity, epoch, key, old, new)` (crud.rs:470, cdc.rs:281)
émet UN événement `Update` par propriété avec `before`/`after` mono-clé. Un
« claim » d'un nœud = la valeur courante de l'une de ses propriétés ; une
contradiction = la valeur qui **oscille** (A→B→A→B… : l'état se dément
lui-même dans la fenêtre).

## 2. Sémantique proposée (v1)

`search_conflicts` = « quels nœuds se contredisent dans le temps ? » :
- fenêtre CDC `[since_ms, now]` (mêmes bornes/gabarit que `search_delta`) ;
- par `(nœud, propriété)` : séquence des valeurs distinctes dans l'ordre des
  événements ; **reversal** = retour à un état antérieur après un état
  différent (`v[t] == v[t−2]` sur la séquence effondrée des valeurs distinctes
  consécutives) — le motif A→B→A… ;
- score de conflit par `(nœud, propriété)` = nombre de reversals ; le classement
  met en tête les couples les plus oscillants ;
- sortie : `(node_id, property, reversals)` (une ligne par couple) — tri par
  reversals décroissant, `limit` ;
- vide = aucune dissonance dans la fenêtre (comportement documenté : les
  conflits sont rares par nature).

Lecture pure de la fenêtre CDC (aucune mutation). Déterministe et testable
synthétiquement (écritures A/B/A → comptage exact).

Surfaces :
- `ObrainDB::search_conflicts(since_ms, limit) -> Vec<ConflictRow>` ;
- `CALL obrain.search_conflicts(since_ms, limit) YIELD node_id, property,
  reversals` — procédure GQL classée LECTURE (liste `PROCEDURES_DE_LECTURE`).

## 3. Coût

O(window) mémoire-bornée (≤ `CDC_RELOAD_CAP` = 100 k événements ; valeurs par
couple dans la fenêtre) + tri O(k log k). Aucun scan store, aucun hook sur les
chemins existants.

## 4. DoD (8 cases)

1. Module `engine/src/database/search_conflicts.rs` (feature `cdc`) — additif
   strict, aucune mutation.
2. `CALL obrain.search_conflicts(since_ms, limit)` enregistrée + classée LECTURE ;
   read-only OK ; erreurs explicites : CDC non armé.
3. Tests : (a) synthèse A/B/A ×3 sur une propriété → reversals exacts (3) ;
   (b) monotonie (valeurs qui changent sans revenir) → 0 ; (c) deux couples
   distincts classés par reversals ; (d) CALL + read-only ; (e) fenêtre sans
   update → vide.
4. Suites : engine 680/724, substrate 803, core 870, search_* précédentes —
   aucune modif de test existant.
5. Golden diff ∅.
6. Bench daté : procédure sur copie runtime (fenêtre réelle) — coût O(window) ;
   comptage des vrais conflits détectés sur la copie (donnée datée).
7. Commit unique réversible + chiffres dans le ledger.
8. Doc + ledger ; pas de déploiement prod.

## 5. Risques / limites (documentées)

- R1 : la détection est **temporelle** (oscillation d'état), pas sémantique
  (pas d'analyse de contenu) — deux textes contradictoires écrits une seule
  fois chacun ne sont PAS détectés (hors périmètre NLP ; le chantier
  s'arrête à la dissonance d'état observable via CDC).
- R2 : `Value` porte `Eq + Hash` → valeurs comparables ; événements triés par
  timestamp (`changes_since_timestamp`).
- R3 : sparse par nature sur données réelles (vide = pas de conflit) — assumé.

## 6. Ordre d'exécution (round suivant)

1. module engine + détection → 2. CALL + LECTURE → 3. tests synthétiques →
4. suites + bench daté (copie runtime) → 5. ledger + commit → 6. proposition
`search_causes/search_effects` (dernière forme).
