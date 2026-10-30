# CLAUSE P4' -- TEXTE FINAL DE DECISION

**Statut : `PROPOSED_NOT_ADOPTED`.** Le protocole v2 reste
`822db99f6c288e346e1dca06c7405ab13a4a06d69bc05beccbe92e333fe5fa3d`.

---

## 1. Propriete PRECEDENTE (P4, v2)

> **P4 -- propriete negative de rotation baseline.** Verdict autorise si :
> rotation **absente** sur la baseline, **avec couverture non nulle** ;
> INCOMPLETE / NOT_CONFIRMED si : couverture nulle.

**Etat : NON EVALUABLE.** Le collecteur f1a filtre sur
`w->type == GGML_TYPE_TQ2_G128` (`f1a-capture.cpp:442`). La baseline est
quantifiee **Q4_K_M** : aucune cible eligible, donc **couverture nulle**, et une
couverture nulle reste `NOT_CONFIRMED`. **Ajouter des devices ne change pas son
type de poids.**

## 2. Propriete de REMPLACEMENT (P4')

> **P4' -- inatteignabilite du chemin WHT pour la baseline.**
> **Propriete exigee :** aucun chemin de generation de WHT n'est **atteignable**
> pour les poids de la baseline.
> **Methode :** inventorier les **types effectifs** des tenseurs atteignant
> `build_lora_mm`, et etablir qu'aucun n'est `GGML_TYPE_TQ2_G128` ; la branche de
> `llama-graph.cpp:1497` est alors **inatteignable**.
> **Verdict autorise si :** les six conditions ci-dessous sont verifiees.
> **INCOMPLETE / NOT_CONFIRMED si :** une seule manque.

## 3. Les SIX conditions exactes

| # | Condition | Verificateur |
|---|---|---|
| C1 | **registre natif complet** du modele charge, via `llama_internal_get_tensor_map` -- pas un echantillon | `check_p4_structural` |
| C2 | **types REELS** lus sur les tenseurs ; **jamais** deduits du nom `Q4_K_M` | idem (FAIL si deduit) |
| C3 | **aucun poids pertinent** (atteignant `build_lora_mm`) n'est `TQ2_G128` | idem (nomme l'offenseur) |
| C4 | **audit des autres sites de creation WHT** sur le fork qualifie | idem |
| C5 | **identite du code et conditions du chemin** : SHA libllama, `OBRAIN_NO_ROTATION` **absent**, `ctx_type`, `gtype` | idem |
| C6 | **limites de la conclusion enoncees** -- ne peut JAMAIS transformer un FAIL en PASS | idem (teste) |

Verdict global = le **PIRE** des six. Les six verificateurs sont **implementes,
raccordes et testes** (26/26). Resultat serialise sous
`ENGINE008_P4PRIME_CHECKS_v1` avec **`"adopted":false`**.

## 4. Portee STRUCTURELLE, et non-observation

P4' porte sur la **construction du graphe**. Elle **ne revendique AUCUNE
observation runtime directe** : aucun des six verificateurs ne lit une trace
d'execution. Les surfaces f1a elles-memes n'observent pas l'execution ; le
deroule du backend n'est trace par rien.

**Ce que P4' change n'est pas seulement la methode mais la NATURE de la
propriete** : une **INATTEIGNABILITE STRUCTURELLE** remplace une **ABSENCE
OBSERVEE**. L'arbitrage doit le dire explicitement.

## 5. Consequences sur les gates et les rapports

**Si ADOPTEE :**
- P4 devient **evaluable** : le verdict se fonde sur C1-C6, tous a PASS ;
- les rapports portent `ENGINE008_P4PRIME_CHECKS_v1` et `p4_overall` ;
- une limite declaree devient **compatible avec le gel**, puisqu'une propriete
  requise est desormais evaluable ;
- **la deduction d'execution (PROOF-13) reste une deduction** : P4' ne l'eleve
  pas en observation.

**Si REJETEE :**
- P4 reste **NON EVALUABLE** sur la baseline Q4_K_M ;
- une propriete requise n'etant pas evaluable, **le gel reste refuse** -- et ce
  n'est pas un manque de travail technique.

## 6. Adoption != autorisation

**L'adoption du protocole ne vaut PAS autorisation des quatre runs.**
L'autorisation reste un document **externe** distinct, avec ses propres
identites, cree separement. Aucun document de ce type n'existe.

## 7. Conditions : verificateurs prets, resultats NON EXERCES

Les verificateurs sont implementes et testes. **Leurs resultats reels restent
`NOT_EXERCISED`** : aucune acquisition native n'a eu lieu
(`FULL_MODEL_LOAD_COUNT=0`). C1, C2, C3 et C5 rendent donc **UNKNOWN** en
pratique, ce qui est l'etat correct et non un echec.
