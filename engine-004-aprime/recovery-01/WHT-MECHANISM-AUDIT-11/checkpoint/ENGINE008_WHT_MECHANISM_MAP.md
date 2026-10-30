# ENGINE008_WHT_MECHANISM_MAP (WHT-MECHANISM-AUDIT-11)

**Methode :** lecture des sources Q1 `87c42d5f…` + captures historiques. Aucun
modele charge, aucun forward.

---

# 0. CORRECTION EXPLICITE DE FINALIZE-10

**J'AI AFFIRME A TORT, dans FINALIZE-10 :**

> « la rotation est STRUCTURELLEMENT impossible sur cet hote [...] la rotation est
> toujours nulle, et `rotation_applied` ne peut pas devenir vrai »

**C'est FAUX.** Cette affirmation confondait deux mecanismes qui ne partagent que
le **mot** « rotation ». La WHT mesuree par `rotation_applied` **ne depend pas du
nombre de devices**.

Ce que j'avais fait de travers : j'ai lu `rotation = get_il_eff(il) % ud->n_devices`
dans `llama-model.cpp`, j'ai vu `% 1 == 0` avec un seul GPU, et j'ai **transfere**
cette conclusion a `rotation_applied` sans jamais lire le chemin qui la produit.
La consigne l'interdisait mot pour mot : « Ne pas etablir leur equivalence depuis
le mot "rotation" ». C'est exactement l'erreur que j'ai commise.

**CORRECTIONS QUI EN DECOULENT, et qui annulent FINALIZE-10 §3 :**
- ~~`CANDIDATE_WHT_POSITIVE_PROOF_ASSESSABLE=NO`~~ -> **l'obstacle invoque n'existe pas.**
- ~~V-a (executer sur >= 2 devices)~~ -> **sans objet. Aucun changement de machine
  n'est demande.** La consigne finale previent : « Ne pas demander un changement de
  machine ou de protocole sur la base d'une equivalence de mecanismes non
  demontree. » Je n'en demande aucun.
- ~~L'hote mono-GPU est un bloqueur de P3~~ -> **retire.**

**CE QUI RESTE VRAI de FINALIZE-10 :** la chaine WHT n'est pas raccordee
(`GRAPH_ROLE_RELATIONSHIPS.csv`, `TARGET_EFFECT_PATH.csv`, `RESIDENCY_MANIFEST.csv`
sans parseur ; `rotation_applied` jamais lu). Ce constat, lui, tient : il portait
sur des faits de raccordement, pas sur une inference de mecanisme.

---

# 1. MECANISME A -- rotation par device (`llama-model.cpp`)

```cpp
// :447 commentaire du champ
size_t rotation; // when assigning tensor slices, rotate how the rounding is
                 // done for more even allocation
// :472, :478, :481
rotation = get_il_eff(il) % ud->n_devices;
rotation = hparams.n_layer() % ud->n_devices;
```
- **Structure porteuse** : la configuration de decoupage de tenseurs par device
  (`tensor_config`, retour `{axis, tensor_axis_0, il, rotation}`).
- **Consommateurs** : l'attribution de tranches de tenseurs aux devices, pour
  **repartir l'arrondi** de facon plus uniforme.
- **Effet** : **placement / repartition**, pas calcul d'activation.
- **Lien avec les activations de `build_lora_mm`** : **AUCUN.** Le mecanisme A ne
  touche pas l'activation ; il decide ou va un morceau de poids.

# 2. MECANISME B -- WHT (`llama-graph.cpp:1480-1534`) -- celui que `rotation_applied` mesure

```cpp
ggml_tensor * llm_graph_context::build_lora_mm(ggml_tensor * w, ggml_tensor * cur,
                                               ggml_tensor * w_s) const {
    f1a_capture_ctx * const f1a_cc = f1a_capture_get_ctx();
    ggml_tensor * const f1a_cur_raw = cur;              // activation TELLE QU'ELLE ARRIVE
    if (w->type == GGML_TYPE_TQ2_G128 && getenv("OBRAIN_NO_ROTATION") == nullptr) {
        GGML_ASSERT(cur->ne[0] % OBRAIN_WHT_BLOCK == 0);
        const auto it = obrain_rot.find(cur);           // une seule rotation par activation
        if (it != obrain_rot.end()) { cur = it->second; }
        else {
            ggml_tensor * in = ggml_is_contiguous(src) ? src : ggml_cont(ctx0, src);
            ggml_tensor * x2 = ggml_reshape_2d(ctx0, in, OBRAIN_WHT_BLOCK, ...);
            ggml_tensor * y2 = ggml_wht(ctx0, x2, OBRAIN_WHT_BLOCK);   // LA TRANSFORMEE
            cur = ggml_reshape_4d(ctx0, y2, in->ne[0], in->ne[1], in->ne[2], in->ne[3]);
        }
    }
    ...
    f1a_capture_note(f1a_cc, w, f1a_cur_raw, cur, f1a_cur_cap, f1a_mm_raw, res);   // :1534
}
```

- **Garde 1** : `w->type == GGML_TYPE_TQ2_G128` -- **le type du POIDS**.
- **Garde 2** : `getenv("OBRAIN_NO_ROTATION") == nullptr` -- **l'environnement**.
- **Transformation** : `ggml_wht` (transformee de Walsh-Hadamard par blocs de
  `OBRAIN_WHT_BLOCK`), avec un cache `obrain_rot` pour ne la faire qu'**une fois
  par activation**.
- **`cur_raw` -> `cur_used`** : `f1a_cur_raw` est capture **avant** la
  transformation ; `cur` (transmis comme `cur_used`) est l'activation **apres**.
- **Aucune occurrence de `n_devices` dans ce chemin.**

**Conclusion : `DEVICE_ROTATION_TO_WHT_RELATION=DISTINCT`.**
Ils ne partagent que le mot. Chaine source demontree pour chacun, consommateurs
disjoints, structures porteuses disjointes.

**`WHT_DEPENDENCY_ON_DEVICE_COUNT=NONE_FOUND_IN_AUDITED_PATH`**

---

# 3. Sens exact de `rotation_applied`

```cpp
// f1a-capture.cpp:455-473
void f1a_capture_note(cc, w, cur_raw, cur_used, cur_capture, mm_raw, mm_capture) {
    r.cur_raw_id = (const void *) cur_raw;
    r.cur_eff_id = (const void *) cur_used;
    r.rotation_applied = (cur_raw != cur_used);
```
Appel : `f1a_capture_note(cc, w, f1a_cur_raw, cur, ...)` (`llama-graph.cpp:1534`).

| Question | Reponse |
|---|---|
| Que representent les deux tenseurs ? | `cur_raw` = activation **entrante** ; `cur_used` = activation **effectivement utilisee** par la projection |
| Quelle transformation explique la difference ? | l'insertion de `ggml_wht` par `build_lora_mm` |
| Autre cause possible de difference ? | **OUI, il en existe une** : le cache `obrain_rot`. Si l'activation a **deja** ete transformee pour un consommateur precedent, `cur` vient du cache -- le pointeur differe **encore** de `cur_raw`, donc `rotation_applied` reste vrai, et c'est correct. En revanche, une difference de pointeur **ne prouve pas a elle seule** l'execution : elle prouve que **le graphe construit** fait passer la projection par une activation transformee. |
| Champs role/effet confirmant la WHT | `GRAPH_ROLE_RELATIONSHIPS.csv`, `TARGET_EFFECT_PATH.csv` -- **precisement les surfaces NON RACCODEES** |
| Construction vs lien au calcul soumis | `rotation_applied` releve de la **CONSTRUCTION DU GRAPHE**. Le lien **cible -> calcul soumis** est le maillon **manquant**. |

**`ROTATION_APPLIED_SEMANTICS_VERIFIED=YES`** -- mais la consigne avertit : « Ne pas
promouvoir une difference de pointeurs en preuve d'execution sans les maillons
necessaires. » Les maillons manquants sont nommes ci-dessus.

---

# 4. Preuves Q1 historiques -- la contradiction, et sa resolution

**Sur pieces**, `GRAPH_CAPTURE_MANIFEST.csv` (4 copies, dont
`.../e7b-f1a-q1-forward-v1/on/`) : colonne `rotation_applied` ->
**T=672, F=0, total=672**.

Autorite : `.../on/WHT_RUNTIME_PROOF_AUDIT.txt`
```
PREUVE A (environnement) : Q1_OBRAIN_NO_ROTATION_STATE=UNSET
  verifie par env -u + test ${VAR+x} : UNSET n est pas SET_BUT_EMPTY
PREUVE B (runtime) : TRUE=672  FALSE=0  MISSING=0
CONTROLE CROISE INDEPENDANT : logits Q0 (rotation DESACTIVEE)
  53aa87fb… vs Q1 (rotation ACTIVE) 0c535e26… -> DIFFERENTS
Q1_WHT_ACTIVE_CONFIRMED=YES
```

**LA CONTRADICTION, EXPOSEE :** mon affirmation mono-GPU prevoyait
`rotation_applied` **impossible** ; la capture historique montre **672 vraies sur
0 fausse**, sur **cet hote**, qui n'a **qu'un GPU**.

**RESOLUTION :** il n'y a pas de contradiction a arbitrer -- **mon affirmation
etait fausse**. La WHT ne depend pas de `n_devices` (mecanisme B), et le run
historique l'a bien appliquee. Le calcul `% 1 == 0` etait correct **pour le
mecanisme A**, et sans rapport avec le mecanisme B.

**`Q1_TRUE_ROTATION_CAPTURE_PROVENANCE_VERIFIED=YES`**
**`HISTORICAL_DEVICE_TOPOLOGY=NOT_RECORDED`** -- je n'ai PAS trouve de releve
explicite de la topologie dans les artefacts du run. Je ne l'invente pas. Mais la
topologie actuelle **n'est pas utilisee comme preuve de celle du run** : c'est
l'inverse qui compte, a savoir que la WHT s'est produite **sans** dependre du
nombre de devices, ce qui est etabli par la lecture du mecanisme B.

---

# 5. Baseline : propriete et capacite du collecteur, separees

**Filtre du producteur** (`f1a-capture.cpp:442`) :
```cpp
if (w->type != GGML_TYPE_TQ2_G128) return false;
return g_f1a.eligible_names.find(std::string(w->name)) != g_f1a.eligible_names.end();
```
La baseline est quantifiee **Q4_K_M** -> aucun tenseur `TQ2_G128` ->
**`BASELINE_TQ2_CAPTURE_ELIGIBLE=NO`**, et **ajouter des devices ne change pas son
type de poids**.

**Methode alternative de preuve pour la baseline -- EXAMINEE, NON ADOPTEE :**
la garde du **builder** elle-meme est un fait structurel :
```cpp
if (w->type == GGML_TYPE_TQ2_G128 && getenv("OBRAIN_NO_ROTATION") == nullptr)
```
Si **aucun** poids de la baseline n'est `TQ2_G128`, alors la branche WHT de
`build_lora_mm` est **inatteignable** pour tous ses consommateurs : la WHT
**ne peut pas** y etre appliquee. Cela s'etablirait par l'**enumerateur natif
existant** (types des tenseurs du modele charge), **sans** dependre du collecteur
f1a, donc **sans** dependre de son filtre d'eligibilite.

**LIMITES de cette possibilite, enoncees :**
- elle prouve une **inatteignabilite structurelle**, pas une **absence observee** --
  ce sont deux proprietes differentes, et P4 demande la seconde ;
- elle porte sur la **construction du graphe**, avec le meme maillon manquant que
  §3 (lien cible -> calcul soumis) ;
- elle ne dit rien de l'**execution**.

**Elle n'est PAS adoptee comme remplacement de P4.** Toute modification de la
methode normative devra etre **explicite** (une version de protocole), jamais
glissee a la faveur de cet audit. **`PROTOCOL_CHANGED=NO`.**
