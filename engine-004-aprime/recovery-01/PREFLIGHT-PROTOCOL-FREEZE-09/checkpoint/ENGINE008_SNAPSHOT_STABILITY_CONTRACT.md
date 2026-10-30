# ENGINE008_SNAPSHOT_STABILITY_CONTRACT (ferme Z1)

**Source :** `src/f1a-capture.cpp`, fork Q1 `87c42d5f…`.

## Le producteur, verbatim

```cpp
void f1a_dump_submitted_snapshots() {
    FILE * f = fopen((g_f1a.outdir + "/SUBMITTED_GRAPH_SNAPSHOTS.csv").c_str(), "w");
    if (f == nullptr) return;
    fprintf(f, "submission_id,gf,n_nodes,submission_rc,fingerprint\n");
    for (size_t i = 0; i < g_f1a_snaps.size(); ++i) { ... }
    fclose(f);
}
```
**Un seul site d'appel** dans tout le fork : `f1a-capture.cpp:826`, juste apres
`fclose(m);` dans le chemin d'extraction de capture.

## Reponses aux questions de Z1

| Question | Reponse |
|---|---|
| ecriture et `fclose` synchrones ? | **OUI.** `fopen` -> boucle d'ecriture -> `fclose`, dans UN appel de fonction, sans E/S asynchrone. |
| terminees avant la lecture du harnais ? | **OUI.** Le harnais lit APRES le retour du calcul ; le dump est sur ce chemin. Une fois la fonction rendue, le flux est FERME. |
| une autre ecriture peut-elle encore se produire ? | **Non sur ce fichier** : un seul site d'appel, aucun autre ecrivain. Une lecture concurrente A un dump serait instable, mais le harnais ne lit pas pendant l'extraction. |
| erreurs d'ecriture/fermeture detectables ? | **PARTIELLEMENT.** `if (f == nullptr) return;` traite l'echec d'OUVERTURE en silence (pas de compteur, pas de message). Le retour de `fclose` n'est PAS verifie. `fprintf` n'est pas verifie. |

## Verdict

**SNAPSHOT_POST_CALL_STABILITY=ESTABLISHED**, sous une condition explicite :
aucun dump concurrent, ce que le site unique d'appel rend vrai sur le chemin
actuel. **Aucun delai arbitraire n'est introduit** : la stabilite vient de
l'ordre des appels, pas d'une attente.

Corollaire de contrat : le harnais peut lire les snapshots **apres** le retour
du calcul. Il doit refuser toute conclusion si l'etat rendu est PARTIAL ou
MALFORME.

## Regle de couverture (ferme Z3) -- DECOUVERTE

`f1a_mark_compute_submission()` est le **hook autoritaire** : « c'est ICI, et
nulle part ailleurs, qu'une soumission est prouvee ». Il incremente
`f1a_ctr_submission` et pousse dans `g_f1a_snaps`.

Or `f1a_dump_submitted_snapshots()` ouvre en **`"w"` : ECRASEMENT INTEGRAL**, et
ecrit tout `g_f1a_snaps` en une fois. **La surface n'est donc PAS un journal
append-only : c'est un instantane a DERNIER ECRIVAIN GAGNANT.**

Consequence, etablie par construction et non par speculation sur le run
historique : **la completude de la couverture des snapshots NE PEUT PAS etre
supposee.** Un dump posterieur remplace integralement le precedent. L'absence
d'un snapshot pour une soumission n'est donc **jamais** une preuve la concernant.

C'est ce qui explique 10 evenements `COMPUTE_SUBMIT` pour 2 lignes de snapshot
sans invoquer aucune perte de donnees : le fichier ne represente que l'etat de
l'accumulateur au moment du DERNIER dump.

**SNAPSHOT_COVERAGE_RULE_ESTABLISHED=YES**

## Distinction exigee par la consigne

- **STABILITE DU FICHIER LU** : ETABLIE (post-fermeture, sans ecrivain actif).
- **PRESENCE D'UN SNAPSHOT POUR CHAQUE SOUMISSION** : **NON** etablie, et NON
  exigible de cette surface. Les deux ne sont pas confondues.

## Z2 (rappel, non efface)

Le ledger ne fournit **pas** `n_nodes`. `eligible_target_count` est une AUTRE
grandeur (0 dans la capture historique, contre 4315 dans le snapshot). Aucune
comparaison de comptes ne peut utiliser que des valeurs **effectivement
connues** ; le correlateur ne compare que si les deux cotes sont connus.
