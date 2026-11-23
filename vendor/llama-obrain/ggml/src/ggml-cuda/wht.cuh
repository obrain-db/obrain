#include "common.cuh"

// Un fil par élément du bloc : 1024 est la taille du bloc de la rotation (OBRAIN_WHT_BLOCK), et la
// limite matérielle des fils par bloc CUDA.
#define WHT_MAX_BLOCK 1024

// [obrain] GGML_OP_WHT — la rotation ternaire : transformée de Walsh-Hadamard normalisée, par blocs
// de `ggml_get_wht_block(dst)` le long de ne[0].
//
// Sans cette op, chaque rotation d'un modèle TQ2_G128 retombe sur le CPU : le graphe se fragmente
// (338 segments mesurés sur Vulkan avant la transformée native) et le débit s'effondre.
void ggml_cuda_op_wht(ggml_backend_cuda_context & ctx, ggml_tensor * dst);
