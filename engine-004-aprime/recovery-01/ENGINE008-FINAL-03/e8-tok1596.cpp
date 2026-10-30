// Decodage HORS CALCUL du token 1596, par le tokenizer NATIF de la baseline.
// Aucun forward, aucun contexte : vocab_only.
#include "llama.h"
#include <cstdio>
#include <string>
#include <vector>
int main(int argc, char ** argv) {
    if (argc < 3) { printf("usage: e8-tok1596 <model> <id> [id...]\n"); return 2; }
    llama_model_params mp = llama_model_default_params(); mp.vocab_only = true;
    llama_model * m = llama_model_load_from_file(argv[1], mp);
    if (!m) { printf("MODELE_ILLISIBLE\n"); return 3; }
    const llama_vocab * v = llama_model_get_vocab(m);
    printf("VOCAB_N_TOKENS=%d\n", llama_vocab_n_tokens(v));
    for (int i = 2; i < argc; ++i) {
        const llama_token id = atoi(argv[i]);
        char buf[256]; int n = llama_token_to_piece(v, id, buf, sizeof buf, 0, /*special*/ true);
        if (n < 0) { std::vector<char> big(-n); n = llama_token_to_piece(v, id, big.data(), (int) big.size(), 0, true);
                     if (n >= 0) { printf("ID=%d BYTES=%d PIECE=", id, n);
                         for (int k=0;k<n;++k) printf("%02x", (unsigned char) big[k]); printf("\n"); } continue; }
        printf("ID=%d BYTES=%d PIECE=", id, n);
        for (int k = 0; k < n; ++k) printf("%02x", (unsigned char) buf[k]);
        printf("\nID=%d ESC=", id);
        for (int k = 0; k < n; ++k) { unsigned char c = buf[k];
            if (c=='\\') printf("\\\\"); else if (c=='"') printf("\\\""); else if (c>=32&&c<127) printf("%c", c);
            else printf("\\x%02x", c); }
        printf("\nID=%d TEXT=[", id); fwrite(buf,1,n,stdout); printf("]\n");
        printf("ID=%d IS_EOG=%d\n", id, (int) llama_vocab_is_eog(v, id));
    }
    llama_model_free(m); return 0;
}
