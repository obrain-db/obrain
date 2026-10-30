# ENGINE008_RUNTIME_PROOF_CONTRACT_v2

**Passe :** PROTOCOL-HARNESS-01 — 2026-10-02
**Statut de cette passe :** contrat écrit ; **aucune preuve runtime exercée**.
`HARNESS_PREFREEZE_DECODE_CALL_COUNT=0`, `..._MODEL_FORWARD_CALL_COUNT=0`.

---

## 0. Les quatre statuts — distincts et non interchangeables

| Statut | Signification |
|---|---|
| `EXPECTED` | ce que le protocole exige comme résultat final |
| `METHOD_DEFINED` | la méthode de capture est écrite et exécutable |
| `STATICALLY_SUPPORTED` | les sources et audits recouvrés soutiennent la propriété |
| `RUNTIME_CONFIRMED` | **mesuré à l'exécution**, sur le vrai run |

> **`RUNTIME_CONFIRMED` ne devient jamais `YES` sur la seule base des sources.**
> Dans cette passe, toutes les propriétés sont au plus `STATICALLY_SUPPORTED`.

---

## 1. Documentation MTP — produite par cette passe

Aucun texte MTP antérieur propre à ENGINE-008 n'existait (constat RECOVERY-01).
La documentation est donc **produite ici**, à partir des sources du fork Q1 et
des audits recouvrés. Elle ne prétend pas reprendre un document antérieur.

**Chargement vs exécution — la distinction est structurelle.** Le candidat porte
65 blocs (`qwen35.block_count=65`) ; `blk.64` contient à la fois un bloc
transformer complet et **4 tenseurs NextN** :

```
blk.64.nextn.eh_proj.weight            Q4_K  [10240, 5120]
blk.64.nextn.enorm.weight              F32   [5120]
blk.64.nextn.hnorm.weight              F32   [5120]
blk.64.nextn.shared_head_norm.weight   F32   [5120]
```

Trois notions doivent rester séparées et ne sont **pas** déductibles l'une de
l'autre :

1. **Tenseurs chargés** — `llama_model_load_from_file` lit les 866 tenseurs.
   Un tenseur chargé ne prouve rien sur son exécution.
2. **Couche présente** — `blk.64.nextn.*` existe dans le fichier : fait de
   métadonnée, mesuré.
3. **Couche exécutée** — seul le chemin de graphe effectivement construit le
   détermine. **Aucun flag ni nom de binaire ne le démontre.**

Le mode de chargement du harnais est `vocab_only = true` : aucune pondération
n'est chargée, donc la question ne se pose même pas pendant cette passe.

**Chemin attendu du futur harnais :** `llama_decode` standard, contexte
`LLAMA_CONTEXT_TYPE_DEFAULT`, aucun réglage spéculatif, aucune requête de graphe
NextN/MTP, `blk.64.nextn.*` jamais atteint.

---

## 2. Propriétés à démontrer, avec leur statut

### 2.1 MTP / NextN

| Propriété | Statut cette passe |
|---|---|
| `ENGINE008_STANDARD_DECODER_EXPECTED` | `EXPECTED=YES` |
| décodeur standard effectivement invoqué | `METHOD_DEFINED` ; `STATICALLY_SUPPORTED` |
| absence de décodage spéculatif / MTP configuré | `METHOD_DEFINED` ; `STATICALLY_SUPPORTED` |
| absence de requête de graphe NextN | `METHOD_DEFINED` ; `STATICALLY_SUPPORTED` |
| `ENGINE008_NEXTN_LAYER_PRESENT` | `STATICALLY_SUPPORTED=YES` (métadonnée mesurée) |
| `ENGINE008_NEXTN_LAYER_USED_IN_SCORING_PATH` | `EXPECTED=NO` ; `RUNTIME_CONFIRMED=NO` |
| `ENGINE008_MTP_ACTIVE` | `EXPECTED=NO` ; `RUNTIME_CONFIRMED=NO` |

Résultats exigés **avant admission des scores** :

```text
ENGINE008_STANDARD_DECODER_GRAPH_CONFIRMED=YES
ENGINE008_MTP_ACTIVE=NO
ENGINE008_NEXTN_LAYER_USED_IN_SCORING_PATH=NO
```

### 2.2 WHT / rotation

| Propriété | Statut cette passe |
|---|---|
| `ENGINE008_BASELINE_RUNTIME_ROTATION_EXPECTED` | `EXPECTED=NO` |
| `ENGINE008_CANDIDATE_RUNTIME_ROTATION_EXPECTED` | `EXPECTED=YES` |
| `ENGINE008_CANDIDATE_WHT_RUNTIME_CONFIRMED` | `NO` |
| `OBRAIN_NO_ROTATION` absent (parent **et** enfant) | `METHOD_DEFINED` ; `STATICALLY_SUPPORTED` |

Condition source recouvrée (`Q1_FORWARD_PROTOCOL_v1.md`, SHA vérifié
`55b69719fc78b1d71d66a390d8db7a792bafef003296156bb0fe2c6e1684a8b5`) :

```
WHT_ENABLE_CONDITION_SOURCE  = w->type == GGML_TYPE_TQ2_G128 AND getenv("OBRAIN_NO_ROTATION") == nullptr
WHT_DISABLE_CONDITION_SOURCE = w->type != GGML_TYPE_TQ2_G128 OR  getenv("OBRAIN_NO_ROTATION") != nullptr
```

**`UNSET` est un test de PRÉSENCE.** `OBRAIN_NO_ROTATION=0` serait **incorrect**.
Le wrapper futur utilisera explicitement `env -u OBRAIN_NO_ROTATION`, et le
contrôle se fera par `test ${VAR+x}` — pas par comparaison de valeur.

Pourquoi le candidat est concerné et le baseline non : le candidat porte
**168 tenseurs `TQ2_G128`** (~10,884 milliards de poids) ; le baseline n'en porte
aucun (qtypes `F32, Q8_0, Q4_K, Q6_K`). Le chemin de décodage du candidat passe
donc par des tenseurs dont la sémantique dépend de la rotation repliée hors ligne.

---

## 3. Méthode de capture, sans modification du calcul Q1

Contrainte : **aucune modification du calcul produit**. La preuve ne peut donc
pas reposer sur une instrumentation ajoutée à `libllama`.

Sources de capture autorisées, par ordre de préférence :

1. **Journal runtime du fork** — les logs de construction de graphe du fork Q1
   nomment le type de graphe et les couches atteintes. Capturés par redirection
   de `stderr` vers un fichier, sans toucher au code.
2. **`llama_model_meta_val_str` / métadonnées** — pour la partie « présent ».
3. **Comptage d'invocations par `LD_PRELOAD`** — interception de `llama_decode`
   et des fonctions de graphe, sans modifier le binaire. Un shim en lecture
   seule, hors de l'arbre du fork.
4. **Preuve d'environnement** — `env -u` + `test ${VAR+x}`, sur le parent et sur
   l'enfant, exactement comme la preuve A de Q1.
5. **Contrôle croisé différentiel** — comme la preuve C de Q1 : deux
   configurations produisent des sorties différentes, ce qui démontre l'effet
   réel d'un paramètre sans inspecter le code.

Le précédent Q1 (`WHT_RUNTIME_PROOF_AUDIT.txt`) démontre que les méthodes 4 et 5
sont réalisables : `Q1_ROTATION_APPLIED_TRUE_COUNT=672`, `FALSE=0`, `MISSING=0`,
plus un contrôle croisé par logits différents.

**Portée, sans surinterprétation :** ces preuves Q1 portent sur le **run
qualificatif Q1 de f1a** et son modèle. Elles fournissent la méthode et le
précédent, **pas** une preuve pour le candidat ENGINE-008.

---

## 4. Phase où chaque preuve peut être exercée

| Phase | Preuves exerçables | Calcul modèle |
|---|---|---|
| Préparation (cette passe) | `METHOD_DEFINED`, `STATICALLY_SUPPORTED`, métadonnées | **interdit** |
| Post-freeze, préflight (passe suivante) | A (environnement), MTP statique runtime, chargement complet | autorisé **si** l'instruction le permet |
| Premier preflight post-freeze | WHT `rotation_applied`, contrôle croisé différentiel | idem |
| Avant premier token scoré | toutes les propriétés §2 exigées `YES`/`NO` | idem |

**Aucune gate de préparation n'exige le calcul modèle tout en l'interdisant.**
Les propriétés ci-dessus sont donc `METHOD_DEFINED` ici, et leur exercice est
reporté à une phase qui l'autorise.

---

## 5. Conditions de succès et d'arrêt

**Succès :** chaque propriété atteint la valeur `EXPECTED` ci-dessus, mesurée au
runtime, avec les preuves conservées dans le run qualitatif.

**Échec :** si une propriété n'atteint pas sa valeur attendue —
`ENGINE008_MTP_ACTIVE=YES`, `NEXTN_LAYER_USED_IN_SCORING_PATH=YES`,
rotation candidat `NO`, ou `OBRAIN_NO_ROTATION` présent — alors
**l'acceptation des résultats concernés est interdite**. Le run est arrêté, la
gate est nommée précisément, et aucun résultat n'est publié comme valide.

Aucun résultat partiel ne peut être promu en résultat accepté.
