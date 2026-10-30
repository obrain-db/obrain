# ENGINE008_RESUME_PROMPT — PREFLIGHT-BUILD-01 (reprise autonome)

> Prompt autonome. Le coller tel quel suffit à reprendre, sans autre contexte.
> Passe interrompue par limite de contexte — **pas** un blocage technique.

---

## Contexte minimal

ENGINE-008 évalue deux modèles GGUF (baseline Q4_K_M, candidat OBRAIN-v3)
sur un corpus TASK de 2 698 items, avec un harnais offline C++ lié à une
`libllama` qualifiée.

- **Hôte des autorités et de l'exécution :** DGX `spark-c578`, alias ssh `dgx`
  (192.168.1.7). Clé `~/.ssh/id_ed25519`, shell complet.
- **Hôte du checkpoint :** n3, `~/projects/banc-obrain/engine-004-aprime/`.
- **Fork Q1 :** `/home/triviere/projects/llama-obrain-cuda-f1a` (branche
  `feat/cuda-tq2g128`, HEAD `8447604f7f0c3b48a77be46de2466a5b23e27826`).
  **Seul `build-cuda` correspond à Q1**, identifié par sa provenance.
- **Espace de travail :**
  `/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/protocol-harness-01`
- **Arbre d'autorité :**
  `/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/engine-008-quality-v1`

## Interdits absolus

`llama_decode`, `llama_encode`, prefill, forward pass, logits modèle,
génération, NLL/PPL, scoring qualité, benchmark long contexte.
Ne modifier ni `libllama` Q1, ni les GGUF, ni les corpus, ni les scorers,
ni les seuils. Ne pas reconstruire `libllama`.

## Autorités (ne jamais compléter un préfixe)

```
QUALITY_BASELINE_MODEL_SHA256=31629f53165ab6a7dad8c9847dcfd1fdf55829dac1e6e748f4a68581b0033d34
QUALITY_CANDIDATE_MODEL_SHA256=1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937
Q1_QUALIFIED_LIBLLAMA_SHA256=87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2
ENGINE008_EFFECTIVE_CHAT_TEMPLATE_SHA256=c3cf9e34abf4f9e36c2d72165aa9c132d3e2a725b6c2586aaa3a8af9d7a81041
CANDIDATE_EMBEDDED_CHAT_TEMPLATE_SHA256=12827f24b742ea4e80cdc12dbcf9622227056b9f797252a3149263d4f9aaadce
TASK_EVAL_DATA_CONTENT_TREE_SHA256=c30d71d84712fae390be582111f0473c87ccaaad7411e1ef99b97689d69ab84a
TOKEN_EVAL_CONTENT_TREE_V2_SHA256=0ce4512e9d73b5e1cc3beddd2222e9ad7a96592508f10930515ef8b4276cf5dd
TASK_PROMPT_CONTRACTS_V1_SHA256=fa30f16d95a564491e6d02666350ba783d6f44493ea5d04cef024e92ef0c3bc6
ENGINE008_PROTOCOL_SHA256=c42f485d41214fd9a87b9398c0df5a82965f68eb35e2ee69e17c4db76252e1a3
ENGINE008_FREEZE_MANIFEST_SHA256=855172969ef621fd0223a79d5443dd44c81fd08b329c212af915b55fdcb82a42
HARNESS_SOURCE_SHA256=3e9691918fa5e861862f7068bbb4af0025d8c1ca2e21a978a0cd32c658aeab7d
HARNESS_BINARY_SHA256=011bd43880307fe1271eeb7b7865889a135f2689d6a4c74091177163892a0665
```

## Template commun — décision adoptée

Objectif `CONTROLLED_IDENTICAL_INPUTS`. Le template **baseline** est imposé
**explicitement** aux deux modèles via
`common_chat_templates_init(model, chat_template_override)` — un override non
vide empêche la lecture du template embarqué (`common/chat.cpp:714-729`).
Mesuré : `EMBEDDED_TEMPLATE_OVERRIDDEN_BY_EXPLICIT=YES` sur le candidat.

## Le déblocage de cette passe (à réutiliser tel quel)

**Capture WHT — instrument natif Q1, déjà compilé, inerte par défaut :**

```
src/f1a-capture.cpp:473     r.rotation_applied = (cur_raw != cur_used);
OBRAIN_F1A_CAPTURE=1  OBRAIN_F1A_OUTDIR=<dir>  OBRAIN_F1A_RUN_ID=<id>
OBRAIN_F1A_INVENTORY=<...>
sorties : EVENT_LEDGER.csv, GRAPH_CAPTURE_MANIFEST.csv, GRAPH_ROLE_RELATIONSHIPS.csv
colonnes ledger : run_id,event_sequence_id,event_kind,decode_step,
  compute_submission_id,graph_ptr,graph_fingerprint,capture_epoch_id,
  eligible_target_count,captured_target_count
```

C'est **le même instrument** qui a produit `Q1_ROTATION_APPLIED_TRUE_COUNT=672`.
**Capture MTP :** `f1a_mark_compute_submission(gf, rc)` (`f1a-capture.h:69`)
donne `COMPUTE_SUBMIT` + `graph_fingerprint` du graphe **soumis**. NextN est un
concept de premier ordre : `t_h_nextn` (`llama-graph.cpp:1271`), `n_layer_nextn`
(`:1407`), `LLM_TENSOR_NEXTN_*` (`llama-model.cpp:1476`).

**Limite à ne pas masquer :** ces instruments observent la **construction du
graphe, la soumission et la synchronisation** — pas l'exécution du noyau. Une
couverture nulle (`captured_target_count==0`) doit donner `NOT_CONFIRMED`,
jamais « absence prouvée ».

## Travail restant, dans cet ordre

1. **W1** — Étendre `engine008-quality-eval` d'un mode `preflight` : chargement
   **complet** (pas `vocab_only`), contexte neuf, prefill, boucle de decode
   bornée à 8 tokens, `argmax` direct avec égalité → plus petit token ID,
   contrat d'arrêt (`GENERATE_STOP_TOKEN_IDS=[248046]`), compteurs **distincts**
   (tentatives refusées / `llama_decode` réels / forwards / tokens / arrêts),
   garde refusant **avant** chargement complet, contexte et forward.
   Source actuelle :
   `$FORK/tools/engine008-quality-eval/engine008-quality-eval.cpp` (611 lignes).
   Compilation hors arbre CMake :
   `g++ -std=c++17 -O2 -I$FORK/include -I$FORK/common -I$FORK/ggml/include -I$FORK/vendor <src> -o <out> -L$FORK/build-cuda/bin -lllama-common -llama -lggml -lggml-base -Wl,-rpath,$FORK/build-cuda/bin`
   **Attention** : `common/chat.h` définit déjà `using json = nlohmann::ordered_json` —
   ne pas redéclarer. Pas de paire de substitution dans un littéral étroit.
   Résoudre `/proc/self/exe` **dans le parent** (readlink), jamais dans un shell.
2. **W2** — Activer/parser la capture f1a (les deux designs sont écrits).
3. **W3** — Tests synthétiques des collecteurs : positif / négatif / incomplet.
4. **W4** — Selftest du contrat d'arrêt (EOS, limite, off-by-one).
5. **W5** — Fermer B4 : `ENGINE008_RENDERER_QUALIFICATION_CLOSURE_v1.md`.
6. **W6** — Émettre v1.1 (protocole, source map, contrat de preuves, autorité
   renderer, identité du harnais, manifeste de gel, plan prévisionnel).
7. **W7** — Vérifier checkpoint et correspondances de hashes.

## État des livrables

Écrits : `ENGINE008_WHT_CAPTURE_DESIGN.md`, `ENGINE008_MTP_CAPTURE_DESIGN.md`,
`ENGINE008_PREFLIGHT_BUILD_STATUS.txt`, le présent prompt.
Non produits : closure renderer, harnais étendu, rapports de tests, v1.1,
plan prévisionnel.

`PASS_STATUS=INCOMPLETE_CONTEXT_LIMIT`. Aucune gate scientifique n'a échoué.
Ne pas écrire `BLOCKED_TECHNICAL` pour une interruption de travail.
