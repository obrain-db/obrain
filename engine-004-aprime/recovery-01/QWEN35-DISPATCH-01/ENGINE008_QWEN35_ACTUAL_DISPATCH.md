# ENGINE008_QWEN35_ACTUAL_DISPATCH

**Passe :** QWEN35-DISPATCH-01 — 2026-10-02 | audit statique, aucun forward.

## 0. Correction de portee du rapport precedent

Mes recherches des passes QWEN35-BUILDER-01 etaient limitees a `src/llama-graph.cpp`
et `.h`. **C'etait une erreur de perimetre.** Le constructeur specifique existe :
**`src/models/qwen35.cpp`** (658 lignes), sous `src/models/` — repertoire que je
n'avais pas ouvert.

Ce qui reste vrai de la passe precedente :
- `layer.nextn.*` n'apparait pas dans `llama-graph.cpp` : OBSERVATION CONSERVEE,
  mais elle ne portait que sur le fichier PARTAGE.
- `embeddings_nextn=false` etablit le resultat des lectures conditionnelles
  (`llama-context.cpp:1874`, `:2304`) : CONSERVE.
- Mais cette garde ne prouve pas, a elle seule, qu'aucun etat n'est PRODUIT dans
  le constructeur : c'etait une sur-extension. Corrige ci-dessous.
- A3 (« construction par llama-graph.cpp ») etait une HYPOTHESE : elle est
  remplacee par le dispatch reel etabli ici.
- Le statut d'exclusion etait `UNRESOLVED` : il est desormais TRANCHE.

Les artefacts de QWEN35-BUILDER-01 sont conserves intacts.

## 1. P1 — le dispatch reel, de bout en bout

**Site unique et explicite**, `src/models/qwen35.cpp:129-134`, verbatim :

```cpp
std::unique_ptr<llm_graph_context> llama_model_qwen35::build_arch_graph(const llm_graph_params & params) const {
    if (params.gtype == LLM_GRAPH_TYPE_DECODER_MTP) {
        return std::make_unique<graph_mtp>(*this, params);
    }
    return std::make_unique<graph>(*this, params);
}
```

Chaine complete :
1. architecture GGUF `qwen35` -> `LLM_ARCH_QWEN35` (`llama-arch.cpp`, `llama-vocab.cpp`) ;
2. `src/models/qwen35.cpp` definit les DEUX classes de graphe de cette architecture :
   - `llama_model_qwen35::graph`      (chemin STANDARD, `:137`)
   - `llama_model_qwen35::graph_mtp`  (tete MTP, `:500-501`)
3. selection par `params.gtype` dans `build_arch_graph` (`:129-134`) ;
4. `gtype` provient de `ctx_type_to_graph_type(cparams.ctx_type)`
   (`llama-context.cpp:78-85`), consomme en `:2264`, `:2869`, `:3868` ;
5. **le harnais** construit son contexte avec `llama_context_default_params()` et
   ne modifie pas `ctx_type` -> `LLAMA_CONTEXT_TYPE_DEFAULT` ->
   `LLM_GRAPH_TYPE_DEFAULT` -> **classe `graph`**.

**Le repertoire `src/models/` appartient bien au build** : il contient un
`CMakeLists.txt` et `qwen35.cpp` y figure aux cotes des autres architectures.

Prefill ET decode : le choix du constructeur passe par le meme
`build_arch_graph` ; les points de consommation de `gtype` sont communs aux deux
chemins (`:2264` pour le traitement d'ubatch, `:2869`/`:3868` pour les
parametres de graphe de prefill et de decode).

## 2. Le fait decisif : DEUX classes, et une seule consomme les poids NextN

`grep -n "nextn" src/models/qwen35.cpp` :

| Ligne | Contenu | Classe |
|---|---|---|
| 113-118 | creation des tenseurs `layer.nextn.{eh_proj,enorm,hnorm,embed_tokens,shared_head_head,shared_head_norm}` | **load_arch_tensors** (chargement) |
| 157 | commentaire : « MTP/NextN layers are loaded as extra decoder blocks **but not executed in the main pass**. » | — |
| 535-565 | `build_lora_mm(layer.nextn.eh_proj, ...)`, `build_norm(h_embd, layer.nextn.hnorm, ...)`, `build_norm(tok_embd, layer.nextn.enorm, ...)`, `layer.nextn.embed_tokens ? ... : model.tok_embd` | **`graph_mtp`** (`:500+`) |

**Les consommateurs de poids `layer.nextn.*` sont tous dans `graph_mtp`.**
La classe standard `graph` n'en consomme AUCUN.

## 3. L'etat auxiliaire : produit, et ce qu'il est exactement

Dans la classe **standard**, `:225-226` :

```cpp
    cur = build_norm(cur, model.output_norm, nullptr, LLM_NORM_RMS, -1);
    cb(cur, "h_nextn", -1);
    res->t_h_nextn = cur;
```

**M3 est resolu : le setter de `t_h_nextn` existe**, ici (`:226`) et dans
`graph_mtp` (`:645`). Mon `grep "t_h_nextn\s*="` limite au repertoire `src/*.cpp`
ne l'avait pas vu : un `grep` restreint ne couvre pas toutes les formes.

**Nature exacte de cet etat** : dans le chemin standard, `t_h_nextn` recoit le
**dernier etat cache du tronc**, apres `output_norm`. Ce n'est PAS une operation
NextN : c'est la sortie du tronc, **publiee** pour qu'un futur passage MTP puisse
la consommer. Le chemin standard ne consomme rien de NextN ; il **offre** son
etat. La distinction est essentielle et je la maintiens.

Export : `:228` `if (!cparams.embeddings_nextn_masked && inp_out_ids)`, et
`llama-context.cpp:1874`/`:2304` gatent la lecture hote par
`cparams.embeddings_nextn`, **false par defaut** (`:151`) et non modifie par le
harnais. L'etat est donc **produit dans le graphe mais NON exporte** vers l'hote.

## 4. Les cinq etats, distingues (§3)

| Etat | Verdict |
|---|---|
| tenseurs NextN **charges** | **OUI** — `qwen35.cpp:113-118` ; metadonnees candidat : 4 tenseurs `blk.64.nextn.*` |
| operations NextN **construites** | **NON** dans le chemin standard — consommateurs tous dans `graph_mtp` |
| operations NextN **incluses dans le graphe soumis** | **NON** — aucune construite, donc rien a inclure |
| etat auxiliaire **produit** | **OUI** — `res->t_h_nextn = cur` (`:226`), dernier etat du tronc |
| etat auxiliaire **exporte** | **NON** — `embeddings_nextn=false` |
| operations **executees** | **NON ETABLI** — hors portee, aucun forward |

## 5. Preuve conditionnelle

**Sous ces conditions :**
- A1 `ctx_type = LLAMA_CONTEXT_TYPE_DEFAULT` -> `LLM_GRAPH_TYPE_DEFAULT` ;
- A2 `embeddings_nextn = false` ;
- A3 selection par `build_arch_graph` (`qwen35.cpp:129-134`) de la classe
  `llama_model_qwen35::graph` (chemin standard) ;

**alors** le graphe construit ne consomme aucun poids `layer.nextn.*`, et
n'inclut donc aucune operation NextN. L'etat `t_h_nextn` est produit mais
correspond a la sortie du tronc, et n'est pas exporte.

**Cette conclusion ne dit PAS « absence d'execution runtime confirmee ».** Elle
est statique et conditionnelle, et porte sur la CONSTRUCTION du graphe.

## 6. Note utile pour le candidat

`qwen35.cpp:40` : `mtp_only = (n_layer_nextn > 0) && (ml.get_weight("blk.0.attn_norm.weight") == nullptr)`.
Le candidat possede `blk.0.attn_norm.weight` -> `mtp_only = false` : c'est un
MODELE COMPLET avec un bloc MTP, pas un fichier MTP seul. Son tronc est donc
construit normalement.

## 7. Maillons restants

Aucun maillon bloquant ne subsiste pour l'assertion C. Reste a observer au
runtime (P4, hors passe) : les valeurs effectives de A1/A2 sur le vrai contexte
cree, et le `gtype` reellement transmis. **Aucune preuve statique ne devient
`RUNTIME_CONFIRMED`.**
