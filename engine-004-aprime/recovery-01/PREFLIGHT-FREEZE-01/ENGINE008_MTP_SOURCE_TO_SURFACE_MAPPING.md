# ENGINE008_MTP_SOURCE_TO_SURFACE_MAPPING

**Passe :** PREFLIGHT-FREEZE-01 — 2026-10-02
**Verdict :** `MTP_SOURCE_TO_SURFACE_MAPPING=INCOMPLETE`
**`MTP_CLASSIFIER_SEMANTICS_VERIFIED=NO`**

---

## 1. Le défaut, énoncé sans détour

Dans PREFLIGHT-IMPLEMENT-01, mon classificateur NextN cherchait les
sous-chaînes `nextn` / `NEXTN` / `h_nextn` dans `GRAPH_ROLE_RELATIONSHIPS.csv`.

**Le producteur n'émet jamais ces chaînes.**

```
$ grep -n "nextn" src/f1a-capture.cpp
(vide)
```

Mon test synthétique `nextn_present_detected` passait uniquement parce que
**ma propre fixture contenait la chaîne que je cherchais**. C'est un cas
d'école de test qui valide la mauvaise chose : il qualifiait le collecteur
sur une entrée que le producteur réel ne peut pas produire.

La détection était donc **structurellement incapable** de voir NextN. Elle
aurait toujours rendu `NEXTN_ABSENT_WITH_COVERAGE` — y compris si NextN
avait été exécuté. C'est exactement le piège que §4 interdit.

---

## 2. Ce que `GRAPH_ROLE_RELATIONSHIPS.csv` est réellement

En-tête exact, lu dans le producteur (`f1a-capture.cpp:672-674`) **et**
confirmé sur les fichiers réels de Q1 :

```
epoch,target_id,weight_name,sub_at_build,cur_op,cur_src0_is_effective,
mulmat_op,mulmat_src0_is_weight,mulmat_src1_is_cur,mmcap_op,
mmcap_src0_is_mmraw,downstream_reach,role_ok,effect_class
```

C'est la surface **rôle/effet de la WHT** (opérations `cur`/`mulmat`/`mmcap`,
`effect_class`, `downstream_reach`) — **pas** une surface NextN. Mon
classificateur lisait la mauvaise surface pour la bonne question.

---

## 3. Les surfaces f1a réellement produites (`f1a-capture.cpp`)

```
BACKEND_OWNERSHIP_MANIFEST.csv    EPOCH_CLASSIFICATION.csv
EVENT_LEDGER.csv                  GRAPH_CAPTURE_MANIFEST.csv
GRAPH_ROLE_RELATIONSHIPS.csv      PAYLOAD_PROVENANCE.csv
RESIDENCY_MANIFEST.csv            SUBMITTED_GRAPH_SNAPSHOTS.csv
TARGET_EFFECT_PATH.csv
```

**Aucune n'est documentée comme portant une identité NextN.**

`TARGET_EFFECT_PATH.csv` (en-tête `epoch,target_id,weight_name,role,effect_class,
src0_is_weight,src1_is_cur,dst_is_cur,effect_ok`) porte un champ `role` : sa
grammaire de valeurs doit être relevée avant tout usage.

**Candidat le plus prometteur, non encore examiné : `SUBMITTED_GRAPH_SNAPSHOTS.csv`.**
Un instantané du graphe **soumis** est précisément le niveau où la question
« le graphe du décodeur standard construit-il des opérations NextN ? » peut
recevoir une réponse. Il doit être inspecté — en-tête, grammaire, couverture —
avant toute conclusion.

---

## 4. Types d'événements réellement émis (`EVENT_LEDGER.csv`, run Q1 réel)

```
CAPTURE_BEGIN  CAPTURE_END  CAPTURE_EXTRACT  COMPUTE_SUBMIT
DECODE_BEGIN   DECODE_END   LOGITS_READ      SYNC_COMPLETE
```

Huit types. Mon collecteur n'en comptait que **deux** (`COMPUTE_SUBMIT`,
`SYNC_COMPLETE`). Les six autres — dont `DECODE_BEGIN` / `DECODE_END` /
`LOGITS_READ`, qui portent la notion de **pas de décodage** — sont ignorés.

Extrait réel (run `Q1_ON`) :

```
run_id,event_sequence_id,event_kind,decode_step,compute_submission_id,graph_ptr,graph_fingerprint,capture_epoch_id,eligible_target_count,captured_target_count
Q1_ON,1,COMPUTE_SUBMIT,-1,1,0xfbdeec9f3030,bfb9b6dbfa65aafb,NA,0,0
Q1_ON,2,SYNC_COMPLETE,-1,1,0xfbdeec9f3030,bfb9b6dbfa65aafb,0,0,0
Q1_ON,3,CAPTURE_BEGIN,-1,1,0xfbdeec9f3030,bfb9b6dbfa65aafb,0,1344,0
```

Observations utiles :
- `capture_epoch_id` peut valoir `NA` : la grammaire n'est pas purement numérique.
- `graph_fingerprint` = 16 caractères hexadécimaux ; `graph_ptr` = adresse.
- `eligible_target_count=1344` / `captured_target_count=0` coexistent : **le
  nombre de cibles éligibles n'est pas le nombre de cibles capturées.** Mon
  critère `manifest_rows > 0` comme signe de couverture est donc insuffisant :
  il faut confronter `eligible` et `captured`.

---

## 5. Sens exact des champs — ce qui est établi, ce qui ne l'est pas

| Champ | Sens | Statut |
|---|---|---|
| `COMPUTE_SUBMIT` | graphe **soumis** au backend | établi par le producteur (`f1a_mark_compute_submission`) |
| `SYNC_COMPLETE` | synchronisation atteinte | établi (`:573`) — **distinct** de la soumission |
| `graph_fingerprint` | empreinte du graphe soumis | sens établi ; **ne décrit pas exhaustivement** le contenu du graphe |
| `rc` de soumission | code de retour | **à confirmer** : non relevé dans ce qui précède |
| `role` / `effect_class` | rôle et classe d'effet WHT | sémantique **à relever** avant usage |

**Une soumission n'est pas un achèvement.** Mon collecteur les comptait
séparément, ce qui est correct, mais il ne les **corrélait** pas : aucun
appariement `submission_id` → `SYNC_COMPLETE` n'était vérifié.

---

## 6. Conséquence pour la conclusion négative

Une conclusion d'absence de NextN exige une surface qui **pourrait** le voir.

- La surface utilisée ne le pouvait pas → **toute conclusion antérieure est nulle**.
- `SUBMITTED_GRAPH_SNAPSHOTS.csv` est le candidat à examiner.
- Tant que sa grammaire et sa couverture ne sont pas établies :
  `MTP_CAPTURE_COVERAGE_DEFINED=NO`.

**Une surface démontrée non couvrante ne permet pas une conclusion négative** —
et la remplacer par le retrait de la gate est explicitement interdit.

---

## 7. Correction à apporter (non appliquée dans cette passe)

1. Relever l'en-tête et la grammaire de `SUBMITTED_GRAPH_SNAPSHOTS.csv` sur un
   artefact réel.
2. Classer les opérations du graphe soumis par **identité d'opération relevée**,
   jamais par recherche de nom de variable C++.
3. Supprimer la recherche de sous-chaînes `nextn`/`NEXTN`/`h_nextn`.
4. Ajouter un cas de test **« nom trompeur »** : une ligne contenant la chaîne
   `nextn` dans un champ non pertinent ne doit **pas** produire `NEXTN_PRESENT`.
5. Corréler soumission et achèvement par identifiant, et confronter
   `eligible_target_count` à `captured_target_count`.
