// ENGINE-007B-F1A-Q0 — harnais minimal de decodage deterministe.
//
// Le CLI llama-obrain ne convient pas a une qualification : il reste en mode interactif,
// boucle, et applique un template de chat. Ce programme utilise libllama DIRECTEMENT :
//   - une SEQUENCE DE TOKENS FIGEE (teacher forcing : on force la suite, on ne l echantillonne pas)
//   - un pas de decodage par token (n_tokens = 1)
//   - les logits finaux ecrits en binaire
// Il ne fait aucun sampling, aucune generation, aucune conversation.
//
// Usage :
//   f1a-eval <modele.gguf> <fichier_tokens.txt> <sortie_logits.bin>
//
// fichier_tokens.txt : un id de token par ligne (premier = prompt, suivants = suite forcee)
// sortie : entete "F1ALOG1 <n_steps> <n_vocab> <int32>" puis, par pas, n_vocab float32

#include "llama.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstdint>
#include <vector>
#include <string>

int main(int argc, char ** argv) {
    if (argc < 4) {
        fprintf(stderr, "usage: %s <modele.gguf> <tokens.txt> <logits.bin>\n", argv[0]);
        return 2;
    }
    const char * model_path = argv[1];
    const char * tok_path   = argv[2];
    const char * out_path   = argv[3];

    // --- lecture de la sequence de tokens figee ---
    std::vector<llama_token> toks;
    FILE * ft = fopen(tok_path, "r");
    if (!ft) { fprintf(stderr, "tokens illisible : %s\n", tok_path); return 1; }
    long v;
    while (fscanf(ft, "%ld", &v) == 1) toks.push_back((llama_token) v);
    fclose(ft);
    if (toks.size() < 2) { fprintf(stderr, "il faut au moins 2 tokens\n"); return 1; }
    fprintf(stderr, "[f1a-eval] %zu tokens charges\n", toks.size());

    llama_backend_init();

    llama_model_params mparams = llama_model_default_params();
    mparams.n_gpu_layers = 99;
    llama_model * model = llama_model_load_from_file(model_path, mparams);
    if (!model) { fprintf(stderr, "ECHEC chargement modele\n"); return 1; }
    const int n_vocab = llama_vocab_n_tokens(llama_model_get_vocab(model));
    fprintf(stderr, "[f1a-eval] modele charge, n_vocab=%d\n", n_vocab);

    llama_context_params cparams = llama_context_default_params();
    cparams.n_ctx   = 512;
    cparams.n_batch = 512;
    cparams.logits_all = false;      // on ne veut que les logits du dernier token de chaque pas
    llama_context * ctx = llama_init_from_model(model, cparams);
    if (!ctx) { fprintf(stderr, "ECHEC creation contexte\n"); return 1; }

    FILE * fo = fopen(out_path, "wb");
    if (!fo) { fprintf(stderr, "sortie non ouvrable : %s\n", out_path); return 1; }

    const int32_t n_steps = (int32_t)(toks.size() - 1);
    fwrite("F1ALOG1 ", 1, 8, fo);
    fwrite(&n_steps, sizeof(int32_t), 1, fo);
    fwrite(&n_vocab, sizeof(int32_t), 1, fo);

    std::vector<float> logits(n_vocab);

    // --- pas 0 : PREFILL du premier token ---
    // On evalue ensuite UN SEUL token par appel : c est le regime de decodage que 007A mesure.
    {
        llama_batch b = llama_batch_get_one(&toks[0], 1);
        if (llama_decode(ctx, b) != 0) { fprintf(stderr, "ECHEC decode prefill\n"); return 1; }
    }
    // --- pas suivants : teacher forcing, un token a la fois ---
    for (int32_t s = 0; s < n_steps; ++s) {
        // logits du pas courant
        float * l = llama_get_logits_ith(ctx, -1);
        if (!l) { fprintf(stderr, "logits indisponibles au pas %d\n", s); return 1; }
        memcpy(logits.data(), l, (size_t) n_vocab * sizeof(float));
        fwrite(logits.data(), sizeof(float), (size_t) n_vocab, fo);

        if (s + 1 < (int32_t) toks.size()) {
            llama_batch b = llama_batch_get_one(&toks[s + 1], 1);
            if (llama_decode(ctx, b) != 0) { fprintf(stderr, "ECHEC decode pas %d\n", s); return 1; }
        }
        if ((s % 4) == 0) fprintf(stderr, "[f1a-eval] pas %d/%d\n", s, n_steps);
    }
    fclose(fo);

    llama_free(ctx);
    llama_model_free(model);
    llama_backend_free();
    fprintf(stderr, "[f1a-eval] TERMINE : %d pas -> %s\n", n_steps, out_path);
    return 0;
}
