# T3 -- INVENTAIRE AUTORITAIRE, RETROUVE ET FIGE

## Le fichier

```
/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/
  e7b-f1a-q1-forward-v1/Q1_WHT_ELIGIBLE_INVENTORY.csv
```

**INVENTORY_SHA256=e4c96b6e9b86be307fd90a0e3e51c2c42b55d091286aac13a5a7b8e94c9398c1**

**Confrontation a l'autorite :** le fichier voisin
`Q1_WHT_ELIGIBLE_INVENTORY.sha256` declare

```
e4c96b6e9b86be307fd90a0e3e51c2c42b55d091286aac13a5a7b8e94c9398c1  Q1_WHT_ELIGIBLE_INVENTORY.csv
```

**Concordance EXACTE** avec le SHA recalcule. L'identite est donc figee, et
elle vient de l'autorite du run, pas d'un fichier genere par moi.

## Format verifie contre le producteur

```
tensor_name,role,block,ggml_type,K_logical,N_rows,blocks_per_row,episode_family,episode_count,offset
blk.8.attn_gate.weight,attn_gate,8,TQ2_G128,5120,6144,40,E5,5,3516466432
blk.8.ffn_gate.weight,ffn_gate,8,TQ2_G128,5120,17408,40,E5,5,3611022592
blk.8.ffn_up.weight,ffn_up,8,TQ2_G128,5120,17408,40,E5,5,3634697472
```

| Regle du producteur | Concordance |
|---|---|
| colonne 0 = `tensor_name` | **OUI** -- c'est bien la 1re colonne |
| en-tete detecte par le prefixe `tensor_name` (11 car.) | **OUI** -- la ligne 1 commence par `tensor_name` |
| nom = texte avant la premiere virgule | **OUI** -- les noms sont propres, sans virgule |
| `#` = commentaire | aucun dans ce fichier |
| **doublons -> capture DESACTIVEE** | **aucun doublon** : 169 lignes = 1 en-tete + **168 noms** |

## Perimetre couvert, et concordance independante

**168 noms** dans l'inventaire. Et le manifeste de capture porte
**168 noms de poids DISTINCTS** (mesure anterieure), sur les couches
`blk.8` a `blk.55`, aucun NextN.

**Les deux comptes concordent : 168 = 168.** C'est une concordance INDEPENDANTE
-- l'inventaire est la configuration, le manifeste est l'observation -- et non
une deduction de l'un par l'autre.

L'inventaire porte en outre `ggml_type=TQ2_G128` par ligne, ce qui documente le
type attendu. **Cela ne dispense PAS de lire le type REEL sur les tenseurs** :
la regle reste que le type effectif gouverne la branche, et le harnais le lit
par `llama_internal_get_tensor_map` (verificateur C2 de P4' refuse tout type
deduit d'un nom).

## Perimetre exact, et ce qu'il ne dit PAS

L'inventaire couvre **le perimetre declare par le Q1 de ce run** : les poids
`blk.*` quantifies `TQ2_G128`. Il **ne prouve pas** qu'aucun autre poids du
modele n'est `TQ2_G128` : c'est precisement ce que `tq2_outside_inventory`
mesure, en croisant l'inventaire avec le registre natif complet. Un inventaire
**valide** n'est pas un inventaire **globalement complet**.
