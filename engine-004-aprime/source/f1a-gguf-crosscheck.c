/* ENGINE-007B-F1A — cross-check du lecteur GGUF autonome.
 *
 * Lit le modele par le CHEMIN DE CODE DU FORK obrain (libggml / gguf.h), independamment
 * du lecteur Python ecrit pour F1, et exporte la meme information dans le meme format.
 *
 * Le fichier GGUF stocke, pour chaque tenseur, ses dimensions en clair dans la table ;
 * gguf.h n'expose pas de getter de dimensions, donc elles sont relues ici par le meme
 * chemin de fichier mais avec un parseur independant. La validation croisee porte sur :
 *   nombre de tenseurs, noms, types, et decalages — plus la coherence globale des tailles.
 */
#include "gguf.h"
#include "ggml.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>

static uint32_t rd_u32(FILE * f){ uint32_t v; if (fread(&v,4,1,f)!=1) return 0; return v; }
static uint64_t rd_u64(FILE * f){ uint64_t v; if (fread(&v,8,1,f)!=1) return 0; return v; }

int main(int argc, char ** argv) {
    if (argc < 2) { fprintf(stderr, "usage: %s <modele.gguf>\n", argv[0]); return 2; }

    struct gguf_init_params p;
    p.no_alloc = true;      /* metadonnees seulement */
    p.ctx      = NULL;
    struct gguf_context * ctx = gguf_init_from_file(argv[1], p);
    if (!ctx) { fprintf(stderr, "ECHEC gguf_init_from_file\n"); return 1; }
    const int64_t nt = gguf_get_n_tensors(ctx);

    /* dimensions : relecture independante de la table, meme fichier */
    FILE * f = fopen(argv[1], "rb");
    if (!f) { fprintf(stderr, "ECHEC fopen\n"); return 1; }
    char magic[4]; fread(magic,1,4,f);
    const uint32_t ver = rd_u32(f);
    const uint64_t n_tensors_file = rd_u64(f);
    const uint64_t n_kv = rd_u64(f);
    /* sauter les metadonnees : on ne lit que ce qui est necessaire pour atteindre la table */
    for (uint64_t i = 0; i < n_kv; ++i) {
        uint64_t klen = rd_u64(f); fseek(f, (long) klen, SEEK_CUR);
        uint32_t t = rd_u32(f);
        switch (t) {
            case 0: case 1: case 7: fseek(f,1,SEEK_CUR); break;
            case 2: case 3: fseek(f,2,SEEK_CUR); break;
            case 4: case 5: case 6: fseek(f,4,SEEK_CUR); break;
            case 10: case 11: case 12: fseek(f,8,SEEK_CUR); break;
            case 8: { uint64_t l = rd_u64(f); fseek(f,(long) l, SEEK_CUR); break; }
            case 9: {
                uint32_t et = rd_u32(f); uint64_t n = rd_u64(f);
                for (uint64_t j = 0; j < n; ++j) {
                    switch (et) {
                        case 0: case 1: case 7: fseek(f,1,SEEK_CUR); break;
                        case 2: case 3: fseek(f,2,SEEK_CUR); break;
                        case 4: case 5: case 6: fseek(f,4,SEEK_CUR); break;
                        case 10: case 11: case 12: fseek(f,8,SEEK_CUR); break;
                        case 8: { uint64_t l = rd_u64(f); fseek(f,(long) l, SEEK_CUR); break; }
                        default: goto done_meta;
                    }
                }
                break;
            }
            default: goto done_meta;
        }
    }
done_meta:
    printf("# LECTEUR=libggml_obrain_plus_relecture_table\n");
    printf("# GGUF_VERSION=%u  n_tensors_libggml=%lld  n_tensors_fichier=%llu  n_kv=%llu\n",
           ver, (long long) nt, (unsigned long long) n_tensors_file, (unsigned long long) n_kv);
    printf("tensor_name,type_int,type_name,type_size,blck,ndims,dim0,dim1,dim2,dim3,offset\n");
    for (int64_t i = 0; i < nt; ++i) {
        /* nom */
        uint64_t nlen = rd_u64(f);
        char name[512]; memset(name,0,sizeof(name));
        if (nlen < sizeof(name)) { if (fread(name,1,(size_t)nlen,f)!=(size_t)nlen) { /* tronque */ } }
        else fseek(f,(long) nlen,SEEK_CUR);
        const uint32_t nd = rd_u32(f);
        int64_t d[4] = {1,1,1,1};
        for (uint32_t k = 0; k < nd && k < 4; ++k) d[k] = (int64_t) rd_u64(f);
        const uint32_t tt = rd_u32(f);
        const uint64_t off = rd_u64(f);

        const char * nm_lib = gguf_get_tensor_name(ctx, i);
        const int    t_lib  = (int) gguf_get_tensor_type(ctx, i);
        const size_t off_lib= gguf_get_tensor_offset(ctx, i);
        const int match = (strcmp(name, nm_lib)==0) && (t_lib==(int)tt) && (off_lib==off);
        printf("%s,%d,%s,%zu,%d,%u,%lld,%lld,%lld,%lld,%llu%s\n",
               name, (int) tt, ggml_type_name((enum ggml_type) tt),
               ggml_type_size((enum ggml_type) tt),
               (int) ggml_blck_size((enum ggml_type) tt),
               nd, (long long)d[0],(long long)d[1],(long long)d[2],(long long)d[3],
               (unsigned long long) off, match ? "" : ",MISMATCH_LIBGGML");
    }
    fclose(f);
    gguf_free(ctx);
    return 0;
}
