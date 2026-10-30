// ENGINE-007B-F1A-Q0-N3bis — lecteur de ligne INDEPENDANT, via libggml.
//
// Ce programme n utilise AUCUNE fonction du lecteur GGUF actuellement suspecte.
// Il passe par le chargeur libggml du fork, recupere le tenseur par son nom exact,
// puis lit la ligne 0 depuis le POINTEUR DATA du tenseur — et non depuis un offset
// GGUF calcule a la main.
//
// Usage : n3bis-libggml-row-reader <modele.gguf> <nom_tenseur> <ligne> <sortie.bin>

#include "gguf.h"
#include "ggml.h"
#include "ggml-backend.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstdint>
#include <vector>

int main(int argc, char ** argv) {
    if (argc < 5) {
        fprintf(stderr, "usage: %s <modele.gguf> <nom_tenseur> <ligne> <sortie.bin>\n", argv[0]);
        return 2;
    }
    const char * model_path = argv[1];
    const char * tname      = argv[2];
    const int    row        = atoi(argv[3]);
    const char * out_path   = argv[4];

    // --- chargement AVEC allocation des donnees (no_alloc=false) ---
    struct gguf_init_params p;
    p.no_alloc = false;        // on VEUT les donnees
    p.ctx      = nullptr;
    struct gguf_context * ctx = gguf_init_from_file(model_path, p);
    if (ctx == nullptr) {
        fprintf(stderr, "LIBGGML_LOAD_STATUS=FAILED_gguf_init_from_file\n");
        return 1;
    }
    struct ggml_context * gctx = p.ctx;
    if (gctx == nullptr) {
        fprintf(stderr, "LIBGGML_LOAD_STATUS=FAILED_no_ggml_context\n");
        return 1;
    }
    printf("LIBGGML_LOAD_STATUS=OK\n");

    // --- le tenseur par son nom EXACT ---
    struct ggml_tensor * t = ggml_get_tensor(gctx, tname);
    if (t == nullptr) {
        fprintf(stderr, "TENSOR_NOT_FOUND=%s\n", tname);
        return 1;
    }

    const int64_t K = t->ne[0];
    const int64_t N = t->ne[1];
    const size_t  nb0 = t->nb[0];
    const size_t  nb1 = t->nb[1];
    const size_t  row_bytes = (size_t) nb1;      // stride de ligne, donne par GGML
    const size_t  nbytes    = ggml_nbytes(t);

    printf("TENSOR_NAME=%s\n", t->name);
    printf("TENSOR_TYPE=%s (%d)\n", ggml_type_name(t->type), (int) t->type);
    printf("NE0=%lld\nNE1=%lld\nNE2=%lld\nNE3=%lld\n",
           (long long) t->ne[0], (long long) t->ne[1], (long long) t->ne[2], (long long) t->ne[3]);
    printf("NB0=%zu\nNB1=%zu\nNB2=%zu\nNB3=%zu\n", t->nb[0], t->nb[1], t->nb[2], t->nb[3]);
    printf("GGML_NBYTES=%zu\n", nbytes);
    printf("ROW_BYTES=%zu\n", row_bytes);
    printf("DATA_POINTER_NON_NULL=%s\n", t->data != nullptr ? "YES" : "NO");
    printf("BLOCKS_PER_ROW=%lld\n", (long long) (K / ggml_blck_size(t->type)));
    printf("EXPECTED_ROW_BYTES_FROM_METADATA=%lld\n",
           (long long) ((K / ggml_blck_size(t->type)) * 34));

    if (t->data == nullptr) {
        fprintf(stderr, "DATA_POINTER_IS_NULL -- le chargement n a pas alloue\n");
        return 1;
    }
    if (row < 0 || (int64_t) row >= N) {
        fprintf(stderr, "LIGNE HORS BORNES : %d (N=%lld)\n", row, (long long) N);
        return 1;
    }

    // --- extraction de la ligne depuis le POINTEUR DATA ---
    const uint8_t * base = (const uint8_t *) t->data;
    if (t->buffer != nullptr) {
        // si un buffer est attache, passer par l API backend (chemin independant)
        std::vector<uint8_t> tmp(row_bytes);
        ggml_backend_tensor_get(t, tmp.data(), (size_t) row * nb1, row_bytes);
        FILE * f = fopen(out_path, "wb");
        if (f == nullptr) { fprintf(stderr, "sortie non ouvrable\n"); return 1; }
        fwrite(tmp.data(), 1, row_bytes, f);
        fclose(f);
        printf("READ_PATH=ggml_backend_tensor_get\n");
    } else {
        FILE * f = fopen(out_path, "wb");
        if (f == nullptr) { fprintf(stderr, "sortie non ouvrable\n"); return 1; }
        fwrite(base + (size_t) row * nb1, 1, row_bytes, f);
        fclose(f);
        printf("READ_PATH=tensor_data_plus_row_stride\n");
    }

    // --- 34 premiers octets, en distinguant qs et d ---
    const uint8_t * rowp = base + (size_t) row * nb1;
    printf("FIRST_34_BYTES=");
    for (int i = 0; i < 34; ++i) printf("%02x", rowp[i]);
    printf("\n");
    printf("QS_0_31=");
    for (int i = 0; i < 32; ++i) printf("%02x", rowp[i]);
    printf("\n");
    printf("D_32_33=%02x%02x\n", rowp[32], rowp[33]);
    printf("ROW_WRITTEN_BYTES=%zu\n", row_bytes);

    gguf_free(ctx);
    return 0;
}
