#include "common.cuh"

#define MMVQ_MAX_BATCH_SIZE 8 // Max. batch size for which to use MMVQ kernels.

// [obrain] Capacite SPECIFIQUE au type TQ2_G128 : les instantiations 9 a 16 n'existent que pour lui.
//
// Elle ne remplace PAS MMVQ_MAX_BATCH_SIZE, qui continue de decrire le domaine GENERIQUE historique
// (8) : le reste du code ne doit en aucun cas en deduire que tous les formats supportent 16.
//
// Mesure et non extrapolation. Sur les CINQ formes TQ2_G128 reellement presentes dans le modele
// (m=17408/12288/6144/1024 k=5120 et m=5120 k=6144 -- inventaire du GGUF) :
//   * justesse : 20/20 cas PASSENT contre la REFERENCE CPU aux lots 9, 10, 12 et 16, entrees
//     identiques entre les deux chemins, ecart MMVQ/MMQ ~4,5e-3 sans aucune anomalie apres 8 ;
//   * ressources : aucun spill (STACK = 0), registres 137 a 213 de n=9 a n=16, AUCUNE falaise a
//     8 -> 9 (lue dans le binaire, cuobjdump, attribution certaine par nom mangle) ;
//   * end-to-end (sm_121, draft Q4_K_M) : verification 22,4 ms par jeton commis contre 54-56 ms en
//     MMQ pour le meme lot, soit x2,4-2,5, et la discontinuite dm7 -> dm9 disparait (x2,00 a dm=9
//     contre x1,00 avant).
// Domaine 17 a 49 MESURE lui aussi (2026-09) : justesse 165/165 cas contre la reference CPU sur les
// cinq formes, rel_l2 PLAT avec n (4,27e-3 a 4,96e-3, du meme ordre qu a 9-16), aucun spill jusqu a
// 49 (registres satures a 255/fil des le lot 20, sans debordement), streaming gagnant sur 4 formes
// sur 5 jusqu a 49, end-to-end +19 % (n-gram seul) et +28 % (cascade) sur charge repetitive, et
// AUCUNE regression : decode 0,999x, pp512 1,003x (dispersion 2,1 %), prefill b24/b32/b48
// fortement accelere (+89 %/+60 %/+28 %), b16 et b64 neutres.
// Au-dela de 49 : NON valide (ni justesse, ni ressources, ni perf). Ne pas extrapoler.
#define MMVQ_MAX_BATCH_SIZE_TQ2_G128 49

bool ggml_cuda_should_use_mmvq(enum ggml_type type, int cc, int64_t ne11);

// Returns the maximum batch size for which MMVQ should be used for MUL_MAT_ID,
// based on the quantization type and GPU architecture (compute capability).
int get_mmvq_mmid_max_batch(ggml_type type, int cc);

void ggml_cuda_mul_mat_vec_q(ggml_backend_cuda_context & ctx,
    const ggml_tensor * src0, const ggml_tensor * src1, const ggml_tensor * ids, ggml_tensor * dst, const ggml_cuda_mm_fusion_args_host * fusion = nullptr);

void ggml_cuda_op_mul_mat_vec_q(
    ggml_backend_cuda_context & ctx,
    const ggml_tensor * src0, const ggml_tensor * src1, ggml_tensor * dst, const char * src0_dd_i, const float * src1_ddf_i,
    const char * src1_ddq_i, float * dst_dd_i, const int64_t row_low, const int64_t row_high, const int64_t src1_ncols,
    const int64_t src1_padded_row_size, cudaStream_t stream);
