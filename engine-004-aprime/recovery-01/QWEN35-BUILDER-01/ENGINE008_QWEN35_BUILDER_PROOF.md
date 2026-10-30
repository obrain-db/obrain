# ENGINE008_QWEN35_BUILDER_PROOF

**Passe :** QWEN35-BUILDER-01 — 2026-10-02
**Objet :** trancher l'assertion C — le chemin standard construit-il des
opérations NextN, et les inclut-il dans le graphe soumis ?
**Methode :** lecture source uniquement. Aucune modification.

---

## 1. Le dispatch, de bout en bout

```cpp
// llama-context.cpp:78-85
static llm_graph_type ctx_type_to_graph_type(llama_context_type ctx_type) {
    switch (ctx_type) {
        case LLAMA_CONTEXT_TYPE_DEFAULT: return LLM_GRAPH_TYPE_DEFAULT;
        case LLAMA_CONTEXT_TYPE_MTP    : return LLM_GRAPH_TYPE_DECODER_MTP;
    }
    throw std::runtime_error("Unsupported ctx type");
}
```

Consommation : `llama-context.cpp:2264` (`process_ubatch`), `:2869` et `:3868`
(`graph_params`). Defaut observe en `:3973` : `LLAMA_CONTEXT_TYPE_DEFAULT`.

**Le harnais** (`run_preflight`) utilise `llama_context_default_params()` et ne
modifie **pas** `ctx_type`. Le type effectif est donc `LLAMA_CONTEXT_TYPE_DEFAULT`
-> `LLM_GRAPH_TYPE_DEFAULT`. *(Le type effectif runtime restera a observer.)*

## 2. Le fait central : `layer.nextn.*` n'est JAMAIS consomme par le graphe

```bash
grep -n "layer.nextn\|nextn\." src/llama-graph.cpp
1407:    n_layer_nextn    (hparams.n_layer_nextn),
```

**Une seule occurrence**, et c'est un **champ de parametres de graphe** — pas une
consommation de tenseur. Les poids NextN (`blk.<i>.nextn.eh_proj`, `.enorm`,
`.hnorm`, `.shared_head_norm`, `.shared_head_head`) sont **crees et charges** par
`llama-model.cpp` (:1477, :1542, :1479-1480, :1544-1545) mais **ne sont
references nulle part dans la construction du graphe**.

## 3. L'etat auxiliaire NextN : produit seulement sous condition explicite

```cpp
// llama-graph.cpp:1271
    t_h_nextn     = nullptr;
// llama-graph.cpp:1317-1318
    if (t_h_nextn != nullptr) {
        ggml_set_output(t_h_nextn);
```

```cpp
// llama-context.h:426-429 (commentaire)
    // hidden state required by the nextn layers (2-dimensional array)
    // populated ONLY when cparams.embeddings_nextn is enabled AND the model
    // graph sets llm_graph_result::t_h_nextn
    buffer_view<float> embd_nextn = {nullptr, 0};
```

```cpp
// llama-context.cpp:151   (defaut)
    cparams.embeddings_nextn        = false;
```

Et les deux seuls points de consommation :

```cpp
// llama-context.cpp:1874
    auto * t_h_nextn = cparams.embeddings_nextn ? res->get_h_nextn() : nullptr;
// llama-context.cpp:2304
    auto * t_h_nextn = cparams.embeddings_nextn ? res->get_h_nextn()  : nullptr;
```

**Le harnais ne modifie pas `embeddings_nextn`** : il reste `false`. Donc
`t_h_nextn` est nul aux deux points, l'etat auxiliaire n'est ni consomme ni
exporte.

## 4. Les quatre etats, distingues (§5)

| Etat | Verdict | Preuve |
|---|---|---|
| tensor NextN **charge** | **OUI** | metadonnees GGUF du candidat : 4 tenseurs `blk.64.nextn.*` ; creation `llama-model.cpp:1477,1542` |
| operation NextN **construite** | **NON** (sous les conditions §5) | `layer.nextn.*` jamais reference dans `llama-graph.cpp` |
| operation NextN **ajoutee au graphe soumis** | **NON** (memes conditions) | aucune operation construite, donc rien a ajouter |
| etat standard **exporte pour un futur passage MTP** | **NON** | `embeddings_nextn=false` ; `t_h_nextn` nul ; `ggml_set_output` non atteint |
| operation **executee** | **NON ETABLI -- hors portee** | aucun forward ; cette passe ne confirme aucune execution |

## 5. Preuve conditionnelle (formulation exigee par §6)

**Sous ces conditions :**
1. `ctx_type = LLAMA_CONTEXT_TYPE_DEFAULT` (defaut, non modifie par le harnais) ;
2. `embeddings_nextn = false` (defaut `llama-context.cpp:151`, non modifie) ;
3. construction du graphe par `llama-graph.cpp`, ou `layer.nextn.*` n'apparait
   jamais et ou `t_h_nextn` reste nul ;

**alors** les operations NextN pertinentes **ne sont pas incluses** dans le
graphe destine a la soumission, et l'etat auxiliaire NextN n'est **pas** produit.

**Cette conclusion ne dit PAS « absence d'execution runtime confirmee ».** Elle
est statique, conditionnelle, et porte sur la **construction du graphe**, pas sur
l'execution.

## 6. Maillons NON determines -- nommes, pas contournes

1. **Le builder qwen35 n'a pas ete identifie nommement.**
   `grep "llm_build_qwen35\|struct llm_build_qwen"` sur `llama-graph.cpp` et
   `llama-graph.h` est **VIDE**. Je n'ai donc pas lu « le builder qwen35 » : j'ai
   lu le fichier partage de construction de graphe, ou `layer.nextn.*` est absent.
   Si un builder specifique a qwen35 existe ailleurs, il n'a pas ete examine.
2. **`LLM_GRAPH_TYPE_DECODER_MTP` est declare (`llama-graph.h:41`) mais aucun
   dispatch `case LLM_GRAPH_TYPE_*` n'a ete trouve.** Comment le graphe MTP
   differe reste NON DETERMINE.
3. **Aucun builder n'assigne `t_h_nextn` a une valeur non nulle** dans les
   fichiers examines (`grep "t_h_nextn\s*="` ne rend que `= nullptr` et les deux
   lectures gatees). Cela suggere que la capacite d'export de l'etat NextN est
   **inerte dans ce fork**, mais je ne l'affirme pas : le setter peut vivre
   ailleurs, par exemple derriere `llama-context.h:428` (« sets
   llm_graph_result::t_h_nextn »).

## 7. Ce que cette passe n'a pas fait

Aucune modification du calcul, des parametres, du harnais ou des modeles.
Aucun rebuild (aucun code n'a change). Aucun test modele. Aucune nouvelle
observation presentee comme telle.
