# ENGINE008_QUALITY_PROTOCOL_v2

**Version normative DISTINCTE de v1** (`c42f485d41214fd9a87b9398c0df5a82965f68eb35e2ee69e17c4db76252e1a3`,
conservée intacte). v2 **remplace** v1 pour le preflight ; v1 reste l'autorité
historique.

**Statut : REDIGE, NON GELE.** Voir `ENGINE008_PREFLIGHT_FREEZE_DECISION.txt`.

## 1. Perimetre

Preflight d'observation : etablir si le chemin standard de qwen35 est celui
exécuté, et si le bloc NextN/MTP y est inactif. **Aucun scorer, aucun résultat
qualité, aucun long contexte.** Quatre runs bornés.

## 2. Template et renderer (qualifiés)

| Element | Identité |
|---|---|
| Template commun | `c3cf9e34abf4f9e36c2d72165aa9c132d3e2a725b6c2586aaa3a8af9d7a81041` (8952 o) |
| Qualification renderer | `14cbbc1336fe885a796539eb473e85388f54a4e600daae24348aeaeccf2c9bdd` |
| Override template embarqué | `EMBEDDED_TEMPLATE_OVERRIDDEN_BY_EXPLICIT=YES` |
| Fonction de rendu | `common_chat_templates_apply` |

**Perimetre qualifié** : `use_jinja=true`, `add_generation_prompt=true`,
`enable_thinking=true`, `add_bos=false`, `add_eos=false`, un seul message `user`.
**Hors perimetre** : outils, `reasoning_format` non NONE, message system fourni,
multi-tours. Le bloc `system` est **émis par le template**, pas par l'appelant.

## 3. Entrée et configuration

- Message utilisateur : `Réponds uniquement : OK.` — **aucun system fourni**.
- `n_ctx=512, n_batch=512, n_ubatch=512, n_seq_max=1`
- `ctx_type = LLAMA_CONTEXT_TYPE_DEFAULT` (non modifié)
- `embeddings_nextn = false` (défaut `llama-context.cpp:151`, non modifié)
- `vocab_only = false`
- **Maximum 8 nouveaux tokens**, argmax direct, départage au **plus petit ID**.

## 4. Captures

Activées uniquement dans le chemin autorisé, par les activateurs f1a posés
**après** l'autorisation. Destinations isolées par modèle et par run, refus si
non vide. `OBRAIN_NO_F1A...` : `OBRAIN_NO_ROTATION` doit être **UNSET**
(test de PRÉSENCE : la valeur `0` serait INCORRECTE), via `env -u`.

**Contrat de disponibilité** (`ENGINE008_SNAPSHOT_STABILITY_CONTRACT.md`) :
- ledger : `fflush` par événement -> visible dès le retour de l'écriture ;
- snapshots : écriture `"w"` intégrale + `fclose` synchrones, site unique ->
  **stable après le retour** ;
- **la surface snapshots n'est PAS un journal** : c'est un dump à dernier
  écrivain gagnant. **L'exhaustivité ne peut pas être supposée.**
- aucun délai arbitraire ; un état PARTIAL/MALFORME interdit toute conclusion.

## 5. Méthodes de preuve

### 5.1 NextN — forme CONDITIONNELLE (adoptée)
Enumeration exhaustive **écartée** (non obtenable, cf. matrice P6).
Conclusion conditionnelle sur : (1) paramètres transmis et acceptés ;
(2) dispatch statique vérifié ; (3) preuve de soumission et de succès.
**Interdit** : « absence directement observée dans tous les graphes ».

### 5.2 WHT / rotation
Détection de rotation sur le candidat (P3) et propriété **négative** sur la
baseline (P4), cette dernière exigeant une **couverture non nulle** : une
couverture nulle donne `NOT_CONFIRMED`, jamais « absente ».

## 6. Contrat d'arrêt

`GENERATE_STOP_TOKEN_IDS = [248046]` — **repris du protocole gelé, non élargi**.
Raisons : `EOS`, `MAX_NEW_TOKENS`, `ERROR`. Aucun decode après EOS ni après le
8e token. Égalité d'argmax départagée au plus petit ID. Ligne de logits
invalide -> `ERROR`.

**« OK. » et l'émission spontanée d'EOS ne sont PAS des critères de réussite.**

## 7. Plan des quatre runs (repris de `b1821f33…`)

`baseline1 -> candidat1 -> baseline2 -> candidat2`.
Contexte et **processus neufs** par run. Maximum 8 tokens. Les identités doivent
être revérifiées **avant chaque run**.

## 8. Critères de réussite et arrêt de séquence

**Réussite** : aucun binaire/bibliothèque inattendu ; entrée et configuration
conformes ; aucun dépassement de budget ; aucune mutation d'autorité ; MTP
inactif dans le chemin de scoring ; WHT conforme au §5.2 ; contrat d'arrêt
respecté ; reproductibilité des ids entre les deux runs d'un même modèle.

**Arrêt immédiat de la séquence** : mauvais binaire ou bibliothèque ; entrée ou
configuration non conforme ; erreur de calcul ; état invalide ; MTP/speculative
incompatible ; preuve rotation/WHT contradictoire ; dépassement de budget ;
mutation d'autorité ; observation obligatoire absente.

**Un défaut d'observation après calcul n'autorise NI un PASS automatique, NI des
répétitions supplémentaires non autorisées.**

## 9. Compteurs, codes de sortie, rapports

Compteurs exposés : `REAL_LOAD_CALLS`, `REAL_CONTEXT_CREATE_CALLS`,
`REAL_F1A_ENV_CALLS`, `REAL_REGISTRY_CALLS`, `REAL_PREFILL_CALLS`,
`REAL_DECODE_CALLS`, `MODEL_FORWARD_CALL_COUNT`.
Codes : `0` succès · `2` mode inconnu · `3` refus avant tout calcul · `4` refus
avec calcul détecté · `5` autorisation acceptée inattendue.
Rapport sérialisé : schéma `ENGINE008_PIPELINE_REPORT_v1`, portant
`auth`, `capture_dir`, `registry_*`, `ctx_type_transmitted`,
`embeddings_nextn_transmitted`, `gtype_status`, `prefill_windows`,
`decode_windows`, `enumeration_*`, `per_submission`, `per_call`, `identity`,
`fp_verdict`, `fp_read`, `fp_recomputed`, `fp_submission_id`, `fp_n_nodes`,
`fp_candidates`, `fp_gf`, `generated_token_ids`, `stop_reason`, `limits`, et
**`nextn_conclusion = NONE_IN_THIS_REPORT`**.

## 10. Conditions d'arrêt de la séquence (hors run)

Le gel est refusé si une propriété requise n'est pas évaluable. Voir la décision.
