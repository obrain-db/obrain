# DECISION D'ADOPTION -- P4' remplace P4

**Statut : ADOPTEE** par decision explicite du 2026-10-02.
Auteur de la decision : l'operateur humain. Cette adoption est **normative**.

---

## 1. Propriete PRECEDENTE (P4, protocole v2)

> Absence de rotation **observee** sur la baseline, avec **couverture non nulle**.

**Motif du remplacement :** P4 etait **NON EVALUABLE**. Le collecteur f1a filtre
sur `w->type == GGML_TYPE_TQ2_G128` (`f1a-capture.cpp:442`) ; la baseline est
quantifiee **Q4_K_M**, donc **aucune cible eligible**, donc **couverture nulle** --
et une couverture nulle reste `NOT_CONFIRMED`. Le type de poids ne depend pas du
nombre de devices : aucune configuration materielle ne rendait P4 evaluable.

## 2. Propriete de REMPLACEMENT (P4') -- ADOPTEE

> **Inatteignabilite du chemin WHT dans la CONSTRUCTION DU GRAPHE baseline**,
> sous les **six conditions** C1-C6 verifiees.

`P4_PROPERTY=STRUCTURAL_WHT_PATH_UNREACHABILITY`

## 3. Nature du changement -- ACCEPTEE EXPLICITEMENT

| | P4 | P4' |
|---|---|---|
| Ce qui est etabli | une **ABSENCE OBSERVEE** | une **INATTEIGNABILITE STRUCTURELLE** |
| Sur quoi | des observations de capture | la **construction du graphe** |
| Revendication runtime | observation | **AUCUNE** |

**Preuve structurelle CONDITIONNELLE, sans revendication d'absence directement
observee pendant l'execution.** L'operateur a accepte ce changement de nature.

## 4. Les SIX conditions -- maintenues telles quelles

| # | Condition | Implementee | Testee |
|---|---|---|---|
| C1 | registre natif **complet** (`llama_internal_get_tensor_map`), pas un echantillon | oui | oui |
| C2 | **types REELS** lus sur les tenseurs ; **jamais** deduits du nom `Q4_K_M` (FAIL si deduit) | oui | oui |
| C3 | **aucun poids pertinent** (`build_lora_mm`) n'est `TQ2_G128` ; l'offenseur est nomme | oui | oui |
| C4 | **audit des sites de creation WHT** | oui | oui |
| C5 | **identite du code et du chemin** : SHA libllama, `OBRAIN_NO_ROTATION` **absent**, `ctx_type`, `gtype` | oui | oui |
| C6 | **limites explicites** ; ne peut **JAMAIS** transformer FAIL ou UNKNOWN en PASS | oui | oui |

Verdict global = le **PIRE** des six.

## 5. Gates -- raccordement

- `p4_gate = OPEN` si et seulement si les **six** conditions sont `PASS`.
- **Tout `FAIL` ou `UNKNOWN` -> `BLOCKED`**, la limite `P4PRIME_GATE_BLOCKED` est
  publiee, et **la sequence s'arrete** selon le contrat d'arret.
- Le rapport porte `p4_adopted:true`, `p4_gate`, `p4_property`,
  `p4_real_model_conditions:"NOT_EXERCISED"` et l'objet complet
  `ENGINE008_P4PRIME_CHECKS_v1` avec, par condition, `verdict`, `proof`,
  `scope`, `reason`.

## 6. Ce que l'adoption NE fait PAS

**Elle fixe la REGLE, elle ne constitue PAS un PASS des conditions sur un modele
reel.** `P4_REAL_MODEL_CONDITIONS=NOT_EXERCISED` : aucune acquisition native n'a
eu lieu (`FULL_MODEL_LOAD_COUNT=0`). C1, C2, C3 et C5 rendent donc `UNKNOWN` en
pratique, ce qui **bloque la gate** -- comportement correct, et c'est au
preflight autorise de les exercer.

**L'adoption et le gel n'autorisent AUCUN calcul reel.** L'autorisation reste un
document externe, separe, non cree.

## 7. Portee des methodes, maintenue distincte

**Candidate WHT** -- deux formes distinguees :
- enumeration **exhaustive** : ECARTEE (non obtenable : la surface snapshots est
  un dump a dernier ecrivain gagnant, `f1a_last_gf` ne garde que la derniere
  soumission) ;
- forme **conditionnelle** (adoptee) : contexte transmis et accepte + dispatch
  statique verifie + preuve de soumission. Portee : « sous ces conditions, les
  operations NextN pertinentes ne sont pas incluses dans ce graphe ». **Interdit**
  d'ecrire « absence directement observee dans tous les graphes ».

**Methodes gardees distinctes** : observations directes / deductions du contrat
source / limites de couverture / proprietes non confirmees.

## 8. Limites explicitement conservees

- **Le denombrement des operations PRESENTES DANS LE GRAPHE n'est PAS un
  denombrement des EXPERTS EFFECTIVEMENT SELECTIONNES AU RUNTIME.** Le rapport
  porte la limite `GRAPH_OPS_NOT_RUNTIME_EXPERT_SELECTION`. **La fixture T4 ne
  doit pas etre generalisee a cette propriete.**
- L'execution n'est tracee par **aucune** surface auditee : la preuve
  d'execution reste une **deduction** (soumission observee + synchronisation par
  `llama_get_logits_ith` + appartenance a la cloture de la racine des logits).
- Le nombre d'extractions par fenetre reste **verifie au runtime** : la preuve de
  segmentation est **conditionnelle** a une seule extraction par fenetre.

## 9. D1 / D2

**D1 CONSERVE** sur toutes les fenetres du perimetre declare.
**D2 NON ADOPTE.**
