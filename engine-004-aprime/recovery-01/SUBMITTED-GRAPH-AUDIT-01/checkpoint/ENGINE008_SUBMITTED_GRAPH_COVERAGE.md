# ENGINE008_SUBMITTED_GRAPH_COVERAGE

**Passe :** SUBMITTED-GRAPH-AUDIT-01

## 1. Critères retenus (remplacent les anciens, insuffisants)

Les anciens critères (`ledger_rows > 0`, `manifest_rows > 0`, `submit_events > 0`,
`fingerprints` non vide) reposaient sur des **comptes non nuls**. Ils sont
remplacés par des critères fondés sur le schéma réel :

Un snapshot est **inutilisable** si :
- le fichier est absent → `NOT_CONFIRMED_INCOMPLETE_COVERAGE` ;
- l'en-tête n'a pas exactement 5 colonnes → `MALFORMED_CAPTURE` ;
- une ligne n'a pas 5 colonnes → `MALFORMED_CAPTURE` ;
- **un `submission_rc != 0`** → `NOT_CONFIRMED_SUBMISSION_ERROR` ;
- `submission_id` n'est pas unique par ligne → `NOT_CONFIRMED_AMBIGUOUS_MAPPING`.

## 2. Sens exact des événements du ledger

Huit types réellement émis, relevés sur l'artefact Q1 :

| Événement | Sens |
|---|---|
| `COMPUTE_SUBMIT` | graphe **soumis** au backend — **pas** un achèvement |
| `SYNC_COMPLETE` | synchronisation atteinte — **distinct** de la soumission |
| `CAPTURE_BEGIN` / `CAPTURE_END` | bornes d'une époque de capture |
| `CAPTURE_EXTRACT` | extraction des cibles |
| `DECODE_BEGIN` / `DECODE_END` | bornes d'un **pas de décodage** |
| `LOGITS_READ` | lecture des logits |

**Un événement n'implique pas les autres.** En particulier `COMPUTE_SUBMIT`
n'implique pas `SYNC_COMPLETE` : ils sont désormais **comptés séparément** et
la corrélation par `submission_id` reste **à implémenter**.

## 3. Le défaut de l'ancienne couverture, mesuré

Sur le ledger réel du run Q1 :

```
Q1_ON,3,CAPTURE_BEGIN,-1,1,0xfbdeec9f3030,bfb9b6dbfa65aafb,0,1344,0
```

`eligible_target_count=1344` et `captured_target_count=0` **coexistent**.
« Cibles éligibles » n'est donc **pas** « cibles capturées ». Un critère de
couverture fondé sur un compte non nul aurait pu déclarer COMPLETE avec zéro
cible réellement capturée.

## 4. Conséquence

```text
SUBMITTED_GRAPH_COVERAGE_DEFINED=YES      (critères écrits et implémentés)
SUBMITTED_GRAPH_COVERAGE_VERIFIED=NO      (non exercée sur un run autorisé)
```

Les critères sont **implémentés et testés** sur fixtures. Leur exercice sur un
run réel appartient à la passe runtime.
