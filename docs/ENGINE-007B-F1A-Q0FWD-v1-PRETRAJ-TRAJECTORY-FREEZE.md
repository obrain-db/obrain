# ENGINE-007B-F1A-Q0FWD-v1-PRETRAJ — GEL DE LA TRAJECTOIRE

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../qualification/e7b-f1a-q0-forward-v1/`

---

## 0 — Résultat

**CAS A : la trajectoire historique a été récupérée, avec preuve. Le gate est fermé.**

```
TRAJECTORY_MODE=RECOVERED_HISTORICAL
HISTORICAL_TRAJECTORY_IDENTITY_PROVEN=YES
Q0FWD_RUN_ALLOWED=YES

Q0FWD_TOKEN_TRAJECTORY_SHA256=dc7b55dc5e87f7fcc50d2b2af15878cad7d6e5361b8a01836b10a248e7a21096
TRAJECTORY_FROZEN_AT=2026-09-30T21:19:55+02:00
```

Je ne suis pas resté bloqué sur CAS A par principe : je l'ai cherché, je l'ai trouvé, et la preuve
tient. Je n'ai pas eu à basculer vers CAS B.

## 1 — Le harnais, lu dans le source (§3)

`tools/f1a-eval.cpp` (102 lignes) :

```c
while (fscanf(ft, "%ld", &v) == 1) toks.push_back((llama_token) v);   // l.40
const int32_t n_steps = (int32_t)(toks.size() - 1);                    // l.68
llama_batch b = llama_batch_get_one(&toks[0], 1);                      // l.75  PREFILL
for (int32_t s = 0; s < n_steps; ++s) {
    ... logits ...
    llama_batch b = llama_batch_get_one(&toks[s + 1], 1);              // l.87  INJECTION
}
```

**Le contrat, c'est le code** :

```
INPUT_IDS     = [ toks[0] ]            longueur 1
OUTPUT_IDS[s] = toks[s + 1], s = 0..8  longueur 9
n_steps       = len(toks) - 1
```

**Input et output ne se recouvrent pas** : `toks[0]` est le préfixe, `toks[1..9]` les neuf tokens
injectés. Je n'ai pas dupliqué le premier output dans l'input — votre §12 le demandait explicitement.

Aucun paramètre CLI de teacher forcing, **ni température ni seed** : la séquence vient entièrement du
fichier de tokens.

## 2 — La distinction que le brief ne pouvait pas anticiper

Votre §4 m'orientait vers `r1/tokens-greedy.txt`. **Ce fichier n'est pas la bonne trajectoire.**

```
r1/tokens-greedy.txt   256 tokens  ->  255 pas
log campagne autoritaire :  "[f1a-eval] 256 tokens charges"
                            "[f1a-eval] TERMINE : 255 pas"
```

Pour 9 pas il faut **exactement 10 tokens**. Le fichier réel est :

```
qualification/e7b-f1a-q0-n1bis/Q0_TOKEN_TRAJECTORY.txt
  10 tokens, 43 octets   SHA256=dc8d54a929cfeb42646b099cc3e0af61b8d8a13e5234d748259e225c9d86f44b
  log : "[f1a-eval] 10 tokens charges" puis "[f1a-eval] TERMINE : 9 pas"
```

**Deux fichiers, deux rôles.** `tokens-greedy.txt` est une référence greedy du run à 255 pas ;
`Q0_TOKEN_TRAJECTORY.txt` porte les 9 pas qualificatifs. Les confondre aurait produit une trajectoire
fausse qui aurait *eu l'air* valide.

## 3 — Trois catégories de preuve convergentes (§8 en exige deux)

```
A. SOURCE DU HARNAIS
   f1a-eval.cpp:68, 77-92 : n_steps = len(toks)-1 ; output[s] = toks[s+1]
   -> avec 10 tokens : exactement 9 pas

C. LOG RUNTIME
   e7b-f1a-q0-n1bis/on.txt : "10 tokens charges" puis "TERMINE : 9 pas"

D. FICHIER DIRECTEMENT REFERENCE
   Q0_TOKEN_TRAJECTORY.txt ET son .sha256 sont enregistres DANS le dossier du run
   qualificatif. Deux passes independantes (n1 ET n1bis) ont produit le MEME SHA256.
```

## 4 — La trajectoire gelée (§15)

```
Q0FWD_TRAJECTORY_FORMAT_VERSION=1
Q0_FORWARD_PROTOCOL_VERSION=v1
Q0_FORWARD_PROTOCOL_SHA256=9a467c3a3262709c80dafe4d4855d1477627e7f761498d434c8587d7b2a81a1b
MODEL_SHA256=1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937
TRAJECTORY_MODE=RECOVERED_HISTORICAL
DECODE_STEPS=9
INPUT_COUNT=1
INPUT_IDS=9906
OUTPUT_COUNT=9
OUTPUT_IDS=279,6511,314,9338,369,220,17,1879,323
```

```
UTF8_LF=OUI   UNE_SEULE_NEWLINE_FINALE=OUI
Q0FWD_TOKEN_TRAJECTORY_SHA256=dc7b55dc5e87f7fcc50d2b2af15878cad7d6e5361b8a01836b10a248e7a21096
TRAJECTORY_SHA256_RECHECK=PASS   (relu depuis le disque)
```

## 5 — Validation (§13, §14)

```
VOCAB_SIZE=248320          (lu dans on2.txt, PAS suppose)
INVALID_TOKEN_ID_COUNT=0
STATIC_TEACHER_FORCING_MAPPING=PASS
```

```
STEP_INDEX,CONTEXT_LENGTH_BEFORE,FORCED_TOKEN,CONTEXT_LENGTH_AFTER
0,1,279,2    1,2,6511,3    2,3,314,4     3,4,9338,5    4,5,369,6
5,6,220,7    6,7,17,8      7,8,1879,9    8,9,323,10
```

## 6 — Audit pré-gel (§18)

```
Q0FWD_OFF_LOGITS.bin                        absent
Q0FWD_ON_LOGITS.bin                         absent
Q0FWD_EVENT_LEDGER.csv                      absent
Q0FWD_SENTINEL_VALIDATION.csv               absent
Q0FWD_FULL_NATIVE_REFERENCE_VALIDATION.csv  absent

PRE_TRAJECTORY_FREEZE_QUALIFICATION_ARTIFACT_COUNT=0
DIRECTORY_AUDIT_STATUS=CLEAN
```

Aucun fichier déplacé, aucun supprimé — il n'y avait rien à déplacer.

## 7 — Rapport final (§24)

```
ENGINE007B_F1A_STATUS=Q0FWD_TRAJECTORY_FROZEN
Q0_FORWARD_PROTOCOL_VERSION=v1
Q0_FORWARD_PROTOCOL_SHA256=9a467c3a3262709c80dafe4d4855d1477627e7f761498d434c8587d7b2a81a1b
Q0_FORWARD_PROTOCOL_FROZEN=YES
TRAJECTORY_MODE=RECOVERED_HISTORICAL
HISTORICAL_TRAJECTORY_IDENTITY_PROVEN=YES
TOKEN_SOURCE=qualification/e7b-f1a-q0-n1bis/Q0_TOKEN_TRAJECTORY.txt
TOKEN_SOURCE_SHA256=dc8d54a929cfeb42646b099cc3e0af61b8d8a13e5234d748259e225c9d86f44b
PROSPECTIVE_SELECTION_RULE=NOT_APPLICABLE_RECOVERED_HISTORICAL
VOCAB_SIZE=248320
TEACHER_FORCED_INPUT_COUNT=1
TEACHER_FORCED_OUTPUT_COUNT=9
INVALID_TOKEN_ID_COUNT=0
STATIC_TEACHER_FORCING_MAPPING=PASS
Q0FWD_TOKEN_TRAJECTORY_SHA256=dc7b55dc5e87f7fcc50d2b2af15878cad7d6e5361b8a01836b10a248e7a21096
TRAJECTORY_SHA256_RECHECK=PASS
TRAJECTORY_FROZEN_AT=2026-09-30T21:19:55+02:00
PRE_TRAJECTORY_FREEZE_QUALIFICATION_ARTIFACT_COUNT=0
Q0FWD_RUN_ALLOWED=YES
Q0_FORWARD_STATUS=NOT_EXECUTED
Q1_FORWARD_ALLOWED=NO
Q0_PREDECLARED_STATUS=NOT_EVALUABLE_NO_PREDECLARED_PROTOCOL
Q0_HISTORICAL_STATUS=NOT_PASSED
Q1_HISTORICAL_ALLOWED=NO
HARNESS_DEFECT_56_STATUS=RESOLVED_VERIFIED_BY_TARGETED_PRESERVATION_AND_BITWISE_TRANSPARENCY
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=Q0FWD_V1_EXECUTE_FIRST_QUALIFYING_OFF_ON_RUN
```

## 8 — Livrables réellement créés

```
Q0FWD_TRAJECTORY_HARNESS_SOURCE_TRACE.txt    le contrat lu dans f1a-eval.cpp
Q0FWD_TOKEN_SOURCE_AUDIT.txt                 audit de tokens-greedy.txt ET du vrai fichier
Q0FWD_TRAJECTORY_STATIC_STEP_MAPPING.csv     9 transitions, contexte 1 -> 10
Q0FWD_TOKEN_TRAJECTORY.txt                   la representation canonique, gelee
Q0FWD_TOKEN_TRAJECTORY.sha256                son empreinte
Q0FWD_TOKEN_TRAJECTORY_PROVENANCE.txt        preuves A/C/D, THIS_TRAJECTORY_IS_HISTORICAL=YES
Q0FWD_PRE_RUN_DIRECTORY_AUDIT.txt            dossier propre, 0 artefact de mesure
Q0FWD_TRAJECTORY_FREEZE.txt                  ordre des gels, immuabilite, autorisation
Q0FWD_PRE_STATUS.txt                         synthese et ledger
```

**Non créé** : `Q0FWD_HISTORICAL_TRAJECTORY_RECONSTRUCTION.csv` — inutile, la trajectoire a été
**récupérée** dans un artefact existant, pas reconstruite. Votre §7 dit « seulement si les preuves
sont suffisantes » ; elles l'étaient sous une forme plus forte, un fichier d'époque hashé.

## 9 — Frontière temporelle

```
2026-09-30 20:48:37   Q0FWD_PROTOCOL_v1.txt gele
2026-09-30 21:19:55   Q0FWD_TOKEN_TRAJECTORY.txt gele
<a venir>             premiere execution qualificative OFF/ON
```

**Aucun run OFF, aucun run ON, aucun ledger runtime, aucune validation sentinelle ou 168** dans cette
passe. Je m'arrête au gel, comme votre §22 l'exige.

Le prochain prompt — celui du premier run prospectif réellement qualificatif — peut maintenant être
produit, puisque `Q0FWD_RUN_ALLOWED=YES`.
