# COMMANDES DES QUATRE RUNS ET SCHEMA D'AUTORISATION EXTERNE

**Aucune de ces commandes n'est executee dans cette passe.**
`PREFLIGHT_EXECUTION_ALLOWED=false`.

## Identites GELEES a utiliser

```
HARNAIS_REEL   14ef0c1ab848a8a724ebee85d59733a00affbd05826cf2f1513b4ae1f9ecb791
LANCEUR        43e7c2c5aa8d895a5b415684dc76cf74adeaf686d90d8491e71401b9f4726717
PROTOCOLE_v3   1dc2c3c36329550d29b1abf1ec448f0c82ef6f2d7ab81994deed9320f9d8b7ae
ADOPTION_P4'   5da671af3dfc97db3fb3869325c835cc12ad939dbee241a54a5a63f5f8536f53
PLAN           b1821f335d93464106d8cc7d13d5f167edd1e95d7e1af9cc15a8f747b56ade06
INVENTAIRE     e4c96b6e9b86be307fd90a0e3e51c2c42b55d091286aac13a5a7b8e94c9398c1
TEMPLATE       c3cf9e34abf4f9e36c2d72165aa9c132d3e2a725b6c2586aaa3a8af9d7a81041
RENDERER      14cbbc1336fe885a796539eb473e85388f54a4e600daae24348aeaeccf2c9bdd
MANIFESTE_GEL 3c614f5d4b8bded8ceef5a482d444c4283ed3c667eb65e9765f54f8728c60bfc
```

## Commande par run (processus NEUF a chaque fois)

```sh
env -u OBRAIN_NO_ROTATION \
    OBRAIN_F1A_CAPTURE=1 \
    OBRAIN_F1A_INVENTORY=<INVENTAIRE e4c96b6e...> \
    OBRAIN_F1A_OUTDIR=<DESTINATION ISOLEE, VIDE> \
    OBRAIN_F1A_RUN_ID=<baseline|candidat>-run<N> \
  <HARNAIS 14ef0c1a...> --mode real-preflight \
    --auth-doc        <DOCUMENT D'AUTORISATION EXTERNE> \
    --plan            b1821f335d93464106d8cc7d13d5f167edd1e95d7e1af9cc15a8f747b56ade06 \
    --protocol        1dc2c3c36329550d29b1abf1ec448f0c82ef6f2d7ab81994deed9320f9d8b7ae \
    --binary          14ef0c1ab848a8a724ebee85d59733a00affbd05826cf2f1513b4ae1f9ecb791 \
    --model           <BASELINE Q4_K_M | CANDIDAT OBRAIN-v3> \
    --template        <TEMPLATE c3cf9e34...> \
    --capture-root    <RACINE ISOLEE> \
    --model-tag       <baseline|candidat> --run-tag run<N> \
    --max-new-tokens  8
```

**Ordre impose** : baseline1, candidat1, baseline2, candidat2.
`env -u OBRAIN_NO_ROTATION` : **ABSENCE** exigee -- la valeur `0` serait INCORRECTE.
Arret au premier echec, **aucun retry, aucune cinquieme invocation** (applique par
le lanceur, 23 tests).

## Schema du DOCUMENT D'AUTORISATION EXTERNE (a creer separement)

Format ligne a ligne `cle=valeur`, **refuse si une cle est dupliquee** :

```
well_formed=1
allow_model_compute=1
audience=REAL_PREFLIGHT
binary_sha256=14ef0c1ab848a8a724ebee85d59733a00affbd05826cf2f1513b4ae1f9ecb791
plan_sha256=b1821f335d93464106d8cc7d13d5f167edd1e95d7e1af9cc15a8f747b56ade06
protocol_sha256=1dc2c3c36329550d29b1abf1ec448f0c82ef6f2d7ab81994deed9320f9d8b7ae
```

**Regles du chemin reel, verifiees :**
- une autorisation `audience=MOCK` est **REJETEE** (`REFUSED_MALFORMED_DOCUMENT`) ;
- un document absent -> `REFUSED_NO_DOCUMENT` ;
- **identite de binaire discordante** -> `REFUSED_BINARY_IDENTITY` ;
- plan ou protocole discordant -> refus ;
- `allow_model_compute=0` -> `REFUSED_FLAG_ONLY` ;
- n'importe quel refus laisse `REAL_LOAD_CALLS=REAL_CONTEXT_CREATE_CALLS=
  REAL_F1A_ENV_CALLS=REAL_REGISTRY_CALLS=REAL_PREFILL_CALLS=REAL_DECODE_CALLS=0`
  et rend `exit 3` avec `REFUSED_BEFORE_ANY_COMPUTE=YES`.

**Aucun document de ce type n'a ete cree.** Les seuls existants portent
`audience=MOCK` et sont rejetes.
