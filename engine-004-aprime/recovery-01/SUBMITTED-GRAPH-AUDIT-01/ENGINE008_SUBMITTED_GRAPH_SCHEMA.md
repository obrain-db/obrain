# ENGINE008_SUBMITTED_GRAPH_SCHEMA

**Passe :** SUBMITTED-GRAPH-AUDIT-01 — 2026-10-02

## 1. Producteur exact

`src/f1a-capture.cpp:301-310` — `f1a_dump_submitted_snapshots()`, appelé en
`:826`. Corps intégral :

```cpp
void f1a_dump_submitted_snapshots() {
    FILE * f = fopen((g_f1a.outdir + "/SUBMITTED_GRAPH_SNAPSHOTS.csv").c_str(), "w");
    if (f == nullptr) return;
    fprintf(f, "submission_id,gf,n_nodes,submission_rc,fingerprint\n");
    for (size_t i = 0; i < g_f1a_snaps.size(); ++i) {
        const f1a_sub_snap & s = g_f1a_snaps[i];
        fprintf(f, "%llu,%p,%d,%d,%016llx\n", s.submission_id, s.gf, s.n_nodes, s.rc, s.fingerprint);
    }
    fclose(f);
}
```

## 2. Schéma et grammaire

| Colonne | Sens | Grammaire |
|---|---|---|
| `submission_id` | clé de corrélation avec le ledger | entier croissant |
| `gf` | pointeur du graphe soumis | `%p` (adresse) |
| `n_nodes` | **nombre** de nœuds du graphe soumis | entier |
| `submission_rc` | **code de retour de la soumission** | entier (`0` = succès) |
| `fingerprint` | empreinte du graphe | 16 hexadécimaux (`%016llx`) |

**Ce que la surface EST :** un registre par soumission, portant une **identité**
(fingerprint), un **volume** (n_nodes) et un **statut** (rc).

**Ce que la surface N'EST PAS :** elle **n'énumère aucun nœud**. Pas de nom
d'opération, pas de nom de tenseur, pas de dépendance. « Snapshot » ne signifie
donc **pas** « graphe complet ».

## 3. Captures réelles examinées (conservées brutes)

| Run | Fichier | Lignes | n_nodes | rc |
|---|---|---|---|---|
| Q1 forward (rotation **ACTIVE**) | `e7b-f1a-q1-forward-v1/on/capon/` | 2 | **4315** | 0 |
| Q0 n1 (rotation **DÉSACTIVÉE**) | `e7b-f1a-q0-n1/capon/` | 2 | **3991** | — |

Contenu réel intégral du run Q1 :

```
submission_id,gf,n_nodes,submission_rc,fingerprint
1,0xfbdeec9f3030,4315,0,bfb9b6dbfa65aafb
2,0xfbdeec9f3030,4315,0,bfb9b6dbfa65aafb
```

## 4. Deux faits mesurés, et ce qu'ils autorisent

**Fait 1 — la surface est SENSIBLE au contenu du graphe.**
4315 nœuds avec la rotation active contre 3991 sans : **+324 nœuds**.
La WHT ajoute donc bien des nœuds au graphe soumis, et `n_nodes` **réagit** à
un changement de construction. C'est un contrôle positif réel : la surface
n'est pas inerte.

**Fait 2 — deux soumissions partagent le même `gf` et le même `n_nodes`.**
`submission_id` 1 et 2 portent `0xfbdeec9f3030` / 4315 / `bfb9b6dbfa65aafb`.
Un même graphe est donc soumis plusieurs fois. **`submission_id` n'est pas
l'identité du graphe** ; `fingerprint` l'est, et il est stable ici.

## 5. Conclusion sur la capacité de la surface

La sensibilité de `n_nodes` **ne permet pas** de conclure à l'absence de NextN :
une variation de volume n'identifie pas *quelles* opérations existent. Sans
référence « avec NextN » à comparer, il n'existe aucune inférence légitime.

```text
SURFACE_SUPPORTS_NEXTN_NEGATIVE_CONCLUSION=NO
SURFACE_INSUFFICIENT_FOR_NEXTN_ABSENCE=YES
```
