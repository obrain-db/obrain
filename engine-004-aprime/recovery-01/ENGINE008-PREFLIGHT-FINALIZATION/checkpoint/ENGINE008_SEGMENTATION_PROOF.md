# T2 -- PREUVE DE SEGMENTATION (etablie depuis les appels)

## La chaine, suivie de proche en proche

```
llama_decode                          llama-context.cpp:4691
  -> ctx->decode(batch)                                  :4694
llama_context::decode                                    :2101
  -> balloc->init(batch_inp, vocab, memory, n_embd, n_seq_max, output_all)
  -> const uint32_t n_tokens_all = balloc->get_n_tokens();          :2157
  -> GGML_ASSERT((cparams.causal_attn || cparams.n_ubatch >= n_tokens_all)
                 && "non-causal attention requires n_ubatch >= n_tokens");
  -> while (true) {                                                  :2191
         mctx = memory->init_batch(*balloc, cparams.n_ubatch, output_all);  :2192
         ...
         const auto & ubatch = mctx->get_ubatch();                   :2244
         const auto * res = process_ubatch(ubatch, ...);             :2264
         f1a_capture_end_epoch(...);                                 :1786
     }
Splitters : llama_batch_allocr::split_equal(n_ubatch, sequential, n_keep_tail)
            llama-batch.cpp:510
            llama_batch_allocr::split_seq(n_ubatch)   llama-batch.cpp:681
```

## Ce que cela etablit

1. **L'unite de boucle est le UBATCH**, et sa taille est bornee par
   **`cparams.n_ubatch`** (`memory->init_batch(*balloc, cparams.n_ubatch, ...)`).
2. **Une extraction par iteration** : `f1a_capture_end_epoch` est appele dans la
   boucle (`:1786`), une fois par ubatch.
3. **L'assertion porte sur le NON-CAUSAL** : pour l'attention causale -- notre
   cas -- `n_ubatch >= n_tokens_all` n'est PAS exige. Rien n'oblige donc a
   decouper sous `n_ubatch`.

## Application a la configuration du plan

| Parametre | Valeur |
|---|---|
| `n_batch` | 512 |
| `n_ubatch` | 512 |
| `n_seq_max` | 1 |
| prefill mesure | **59 tokens** (mesure, pas estimation) |
| decode | **1 token** |

**Condition etablie :** un ubatch ne depasse jamais `n_ubatch = 512`. Le prefill
(59) et chaque pas de decode (1) tiennent donc dans **UN seul ubatch**, donc
**UNE extraction** -- a condition qu'aucun splitter ne decoupe SOUS `n_ubatch`.

**Les deux splitter existants** sont `split_equal(n_ubatch, ...)` et
`split_seq(n_ubatch)` : tous deux sont **parametres par `n_ubatch`**, et
`split_seq` decoupe par SEQUENCE. Avec **une seule sequence**, il rend un ubatch.

**SEGMENTATION_PROOF=ESTABLISHED**, sous les conditions NOMMEES :
- attention causale (cas du plan) ;
- **une seule sequence** dans le batch (`n_seq_max=1`, positions explicites) ;
- un prefill de 59 tokens et un decode de 1 token, mesures ;
- `n_ubatch=512`.

**Le controle runtime du nombre d'extractions reste OBLIGATOIRE** : le harnais
compte les evenements `CAPTURE_EXTRACT` du ledger par fenetre et publie
`wht_max_extractions_per_window`. Si une fenetre en montre plus d'une, la
couverture D1 est declaree non obtenable pour cette fenetre -- la condition
ci-dessus est alors REFUTEE par l'observation, ce qui est le comportement voulu.

**Aucune modification** de batching, de CUDA graphs ou de configuration.
