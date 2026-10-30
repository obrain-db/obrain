# ENGINE008_QUALITY_PROTOCOL_v3 -- VERSION NORMATIVE CONSOLIDEE

**Statut : GELE.** Remplace v2 pour le preflight.
v1 (`c42f485d41214fd9a87b9398c0df5a82965f68eb35e2ee69e17c4db76252e1a3`) et
v2 (`822db99f6c288e346e1dca06c7405ab13a4a06d69bc05beccbe92e333fe5fa3d`) sont
**CONSERVES INTACTS** comme autorites historiques.

## 1. Perimetre
Preflight d'observation : etablir si le chemin standard de qwen35 est celui
execute et si le bloc NextN/MTP y est inactif ; etablir l'inatteignabilite du
chemin WHT pour la baseline. **Aucun scorer, aucun resultat qualite, aucun long
contexte.**

## 2. Entree -- MESUREE

| Element | Valeur |
|---|---|
| Message | `Réponds uniquement : OK.` |
| Template commun | `c3cf9e34abf4f9e36c2d72165aa9c132d3e2a725b6c2586aaa3a8af9d7a81041` |
| Qualification renderer | `14cbbc1336fe885a796539eb473e85388f54a4e600daae24348aeaeccf2c9bdd` |
| Rendu | 320 octets, SHA `92eb0cb3a51bbece69aea29afc87553f6b5b5fdd16a1f30485a31dc38ca1c658` |
| Tokens | **59**, IDs SHA `ee1740718fd9fc85bfdec4edac4e561be0336b791d40b2f3514d24d310ece37e` |
| Politique | `add_special=0`, `parse_special=1` |
| Concordance baseline/candidat | **ETABLIE** (rendu ET IDs identiques) |

## 3. Configuration
`n_ctx=512, n_batch=512, n_ubatch=512, n_seq_max=1`,
`ctx_type=LLAMA_CONTEXT_TYPE_DEFAULT`, `embeddings_nextn=false`,
`vocab_only=false`. Maximum **8 nouveaux tokens**, argmax direct, departage au
**plus petit ID**. « OK. » et l'emission spontanee d'EOS ne sont **pas** des
criteres de succes.

## 4. Proprietes et gates

### P1 -- chemin standard / MTP : preuve STATIQUE (`qwen35.cpp:129-134`).
### P2 -- NextN : forme **CONDITIONNELLE** (voir ADOPTION §7).
### P3 -- rotation candidate : `rotation_applied=1` + couverture par **cibles
distinctes**, denominateur = **manifeste**, jamais les lignes deja jointes.
### P4' -- **ADOPTEE** : inatteignabilite structurelle, six conditions C1-C6.
  `p4_gate=OPEN` si et seulement si les six sont PASS. **Tout FAIL ou UNKNOWN
  BLOQUE** et arrete la sequence.
### P5 -- arret : `STOP_TOKEN_IDS=[248046]`, EOS / MAX_NEW_TOKENS / ERROR.
  Aucun decode apres EOS ni apres le 8e token. Logits invalides -> ERROR.
### P6 -- attribution graphe/soumission : **PARTIAL** par construction.
### P7 -- identites : concordance exigee.

## 5. Denominateurs et couverture
**N1** = `intersection(inventaire valide, tenseurs de TYPE REEL TQ2_G128)` --
**independant des captures**. **N2** = couples `(epoch, target_id)` du manifeste.
**N3** = cibles avec relations requises. `N1=UNKNOWN` **interdit la completude**.
**D1 conserve**, **D2 non adopte**.

## 6. Observations / deductions / limites -- TENUES DISTINCTES
- **Observations directes** : `submission_rc`, retour non nul de
  `llama_get_logits_ith` (qui appelle `ctx->synchronize()`), `epoch` et
  `decode_step`, nombre d'extractions par fenetre.
- **Deductions du contrat source** : que la lecture des logits acheve le travail
  soumis ; que la WHT est dans la cloture de la racine des logits
  (`qwen35.cpp:239-241`).
- **Limites de couverture** : snapshots non exhaustifs (dump a dernier ecrivain
  gagnant) ; operations du graphe != experts selectionnes au runtime ; une
  fenetre peut contenir plusieurs extractions.
- **Proprietes NON confirmees** : execution du backend par operation ;
  `gtype` (`GTYPE_DERIVED_FROM_VERIFIED_SOURCE_MAPPING`).

## 7. Plan des quatre runs
`baseline1 -> candidat1 -> baseline2 -> candidat2`, processus et contexte
**neufs** par run, 8 tokens max. Identites reverifiees avant chaque run, arret au
premier echec, **aucun retry, aucune cinquieme invocation** (applique par le
lanceur).

## 8. Arret de sequence
Mauvais binaire ou bibliotheque ; entree ou configuration non conforme ; erreur
de calcul ; etat invalide ; MTP incomptatible ; preuve contradictoire ;
depassement de budget ; mutation d'autorite ; observation obligatoire absente ;
**`p4_gate=BLOCKED`** ; **plus d'une extraction dans une fenetre**.

## 9. Compteurs, codes, rapports
Compteurs : `REAL_LOAD_CALLS`, `REAL_CONTEXT_CREATE_CALLS`, `REAL_F1A_ENV_CALLS`,
`REAL_REGISTRY_CALLS`, `REAL_PREFILL_CALLS`, `REAL_DECODE_CALLS`,
`MODEL_FORWARD_CALL_COUNT`.
Codes : `0` succes / `2` mode inconnu / `3` refus avant tout calcul / `4` refus
avec calcul detecte / `5` autorisation acceptee inattendue.
Rapport : `ENGINE008_PIPELINE_REPORT_v1` + `ENGINE008_INVENTORY_VALIDATION_v1` +
`ENGINE008_EXPECTED_INVOCATIONS_v1` + `ENGINE008_P4PRIME_CHECKS_v1`.

## 10. Adoption != autorisation
**Le gel et l'adoption de P4' n'autorisent aucun calcul reel.** L'autorisation
reste un document externe, separe, non cree dans cette passe.
