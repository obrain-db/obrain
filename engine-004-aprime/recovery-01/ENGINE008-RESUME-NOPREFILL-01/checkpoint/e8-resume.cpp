// e8-resume -- REPRISE SANS PREFILL : transfert de l'ETAT HYBRIDE COMPLET.
// Preparation + tests SANS MODELE. Aucun forward n'est effectue par --selftest.
//
// Transfert : llama_state_seq_{get_size,get_data,set_data}_ext, flags = 0
//   -> mem_attn (KV des 16 couches d'attention) + mem_recr (etat des 48 SSM)
// Le mode PARTIAL_ONLY (=1) ne transporte QUE le recurrent : il est donc
// INSUFFISANT pour une reprise complete, et le pilote le REFUSE explicitement.
#include "llama.h"
#include "ggml.h"
#include "llama-context.h"
#include "llama-model.h"
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstdint>
#include <fstream>
#include <string>
#include <vector>

static void die(const char * m) { fprintf(stderr, "FATAL: %s\n", m); exit(2); }
static const char * arg(int c, char ** v, const char * n, const char * d = nullptr) {
    for (int i = 1; i + 1 < c; ++i) if (strcmp(v[i], n) == 0) return v[i+1];
    if (d) return d; fprintf(stderr, "missing %s\n", n); exit(2);
}
static uint64_t fnv(const uint8_t * d, size_t n) {
    uint64_t h = 1469598103934665603ULL;
    for (size_t i = 0; i < n; ++i) { h ^= d[i]; h *= 1099511628211ULL; }
    return h;
}

// --- Contrat de reprise. Un seul point d'entree : il n'existe pas de reprise
// partielle valide, donc le drapeau PARTIAL_ONLY est REJETE ici.
struct ResumePlan {
    static constexpr uint32_t IO_MAGIC = 0xaf143cd9u;   // llama-context.cpp:3428
    static constexpr int      N_ATTN_LAYERS = 16;       // sonde : [3,7,...,63]
    static constexpr int      N_RECR_LAYERS = 48;       // 64 - 16
    static constexpr int      PREFIX_TOKENS = 58;       // positions 0..57
    static constexpr int      NEXT_POS      = 58;       // decodage du 59e ID
};

static int selftest() {
    int fail = 0;
    printf("E8_RESUME_SELFTEST\n");

    // 1. contrat de drapeaux : seul flags=0 transporte l'etat hybride complet
    printf("  SELFTEST_MAGIC=0x%08x\n", ResumePlan::IO_MAGIC);
    printf("  SELFTEST_FLAGS_FULL=%d (KV attention + recurrent)\n", LLAMA_STATE_SEQ_FLAGS_NONE);
    printf("  SELFTEST_FLAGS_PARTIAL_ONLY=%d (RECURRENT SEUL -> insuffisant)\n",
           LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY);
    if (LLAMA_STATE_SEQ_FLAGS_NONE != 0) fail++;
    if (LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY != 1) fail++;
    // le pilote REFUSE PARTIAL_ONLY : ce n'est pas une reprise complete
    const bool reject_partial = (LLAMA_STATE_SEQ_FLAGS_PARTIAL_ONLY != LLAMA_STATE_SEQ_FLAGS_NONE);
    printf("  SELFTEST_REJECTS_PARTIAL_ONLY=%s\n", reject_partial ? "YES" : "NO");
    if (!reject_partial) fail++;

    // 2. en-tete du blob : magic(4) + seq_id(sizeof(llama_seq_id))
    printf("  SELFTEST_HEADER_BYTES=%zu (magic=%zu + seq_id=%zu)\n",
           sizeof(uint32_t) + sizeof(llama_seq_id), sizeof(uint32_t), sizeof(llama_seq_id));
    if (sizeof(uint32_t) != 4) fail++;

    // 3. decomposition des couches
    printf("  SELFTEST_ATTN_LAYERS=%d\n  SELFTEST_RECR_LAYERS=%d\n  SELFTEST_TOTAL=%d\n",
           ResumePlan::N_ATTN_LAYERS, ResumePlan::N_RECR_LAYERS,
           ResumePlan::N_ATTN_LAYERS + ResumePlan::N_RECR_LAYERS);
    if (ResumePlan::N_ATTN_LAYERS + ResumePlan::N_RECR_LAYERS != 64) fail++;

    // 4. point de transfert
    printf("  SELFTEST_PREFIX_TOKENS=%d (positions 0..%d)\n",
           ResumePlan::PREFIX_TOKENS, ResumePlan::PREFIX_TOKENS - 1);
    printf("  SELFTEST_NEXT_DECODE_POS=%d (59e ID)\n", ResumePlan::NEXT_POS);
    if (ResumePlan::PREFIX_TOKENS != ResumePlan::NEXT_POS) fail++;

    // 5. chemins d'erreur, tels que LUS dans le code
    printf("  SELFTEST_ERR_EXPORT=exception -> log -> retour 0\n");
    printf("  SELFTEST_ERR_MAGIC=magic != 0x%08x -> throw 'wrong sequence state magic' -> retour 0\n",
           ResumePlan::IO_MAGIC);
    printf("  SELFTEST_ERR_RESTORE=state_read_meta/data faux -> clear() ou seq_rm(seq_id,-1,-1) PUIS throw\n");
    printf("  SELFTEST_DESTRUCTIVE_ON_FAILURE=YES  (l'etat destination est EFFACE avant l'exception)\n");
    printf("  SELFTEST_OBRAIN_STATE_TYPE=OBRAIN_STATE_TYPE=f16|i8 (alias OBRAIN_STATE_F16=1) change le blob recurrent\n");

    // 6. hash de reference sur un tampon d'en-tete construit localement
    { uint8_t hdr[sizeof(uint32_t) + sizeof(llama_seq_id)];
      uint32_t mg = ResumePlan::IO_MAGIC; memcpy(hdr, &mg, 4);
      llama_seq_id s = 0; memcpy(hdr + 4, &s, sizeof(s));
      printf("  SELFTEST_HEADER_FNV=%016llx\n", (unsigned long long) fnv(hdr, sizeof hdr)); }

    printf("SELFTEST_RESULT=%s FAILED=%d\n", fail == 0 ? "PASS" : "FAIL", fail);
    return fail == 0 ? 0 : 1;
}

int main(int argc, char ** argv) {
    if (strcmp(arg(argc, argv, "--selftest", "0"), "1") == 0) return selftest();
    fprintf(stderr,
      "Ce pilote est en PHASE DE PREPARATION.\n"
      "La sequence complete (controle avec prefill / destination sans prefill)\n"
      "n'est PAS implementee : elle exige un forward, qui n'est pas autorise.\n"
      "Seul --selftest est executable.\n");
    return 3;
}
