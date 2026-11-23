#include "wht.cuh"

// [obrain] GGML_OP_WHT sur CUDA — le papillon, en mémoire partagée.
//
// Sémantique copiée de `ggml_compute_forward_wht_f32` (ggml-cpu/ops.cpp) : par blocs de `block` le
// long de ne[0], papillon en h = 1, 2, 4, … puis normalisation par 1/sqrt(block). Le CAS GÉNÉRAL
// est traité, `block < ne[0]` compris — contrairement au shader Vulkan, qui ne sert qu'une ligne
// entière (ne[0] == block) et doit donc refuser le reste.
//
// Un bloc CUDA par LIGNE, `block` fils, mémoire partagée de `block` floats, et une boucle sur les
// ne[0]/block groupes de la ligne.
//
// Papillon : à chaque étape h, la paire (j, j+h) est traitée par UN SEUL fil — celui dont l'indice
// tombe dans la première moitié de la paire. Les deux valeurs sont lues avant toute écriture, et
// __syncthreads() sépare les étapes : aucun fil ne lit une case déjà réécrite.
static __global__ void wht_f32_kernel(const float * __restrict__ src,
                                      float       * __restrict__ dst,
                                      const int64_t ne0,
                                      const int     block) {
    extern __shared__ float s[];

    const int64_t row  = blockIdx.x;
    const int     tid  = threadIdx.x;
    const float   norm = 1.0f/sqrtf((float) block);

    for (int64_t b = 0; b < ne0/block; ++b) {
        const int64_t off = row*ne0 + b*block;

        s[tid] = src[off + tid];
        __syncthreads();

        for (int h = 1; h < block; h *= 2) {
            const int j    = tid % (2*h);
            const int base = tid - j;
            if (j < h) {
                const float a = s[base + j];
                const float c = s[base + j + h];
                s[base + j]     = a + c;
                s[base + j + h] = a - c;
            }
            __syncthreads();
        }

        dst[off + tid] = s[tid]*norm;
        __syncthreads();
    }
}

void ggml_cuda_op_wht(ggml_backend_cuda_context & ctx, ggml_tensor * dst) {
    const ggml_tensor * src0 = dst->src[0];

    GGML_ASSERT(src0->type == GGML_TYPE_F32);
    GGML_ASSERT(dst ->type == GGML_TYPE_F32);
    GGML_ASSERT(ggml_is_contiguous(src0));
    GGML_ASSERT(ggml_is_contiguous(dst));
    GGML_ASSERT(src0->ne[0] == dst->ne[0] && src0->ne[1] == dst->ne[1] &&
                src0->ne[2] == dst->ne[2] && src0->ne[3] == dst->ne[3]);

    const int block = ggml_get_wht_block(dst);
    GGML_ASSERT(block > 0 && (block & (block - 1)) == 0); // puissance de deux
    GGML_ASSERT(block <= WHT_MAX_BLOCK);                  // un fil par élément du bloc
    GGML_ASSERT(src0->ne[0] % block == 0);

    const int64_t ne0    = src0->ne[0];
    const int64_t n_rows = ggml_nrows(src0);

    cudaStream_t stream = ctx.stream();

    wht_f32_kernel<<<n_rows, block, block*sizeof(float), stream>>>(
        (const float *) src0->data, (float *) dst->data, ne0, block);
}
