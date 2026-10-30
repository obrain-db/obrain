// ENGINE-005E — sonde de construction : intercepte ggml_mul_mat et compte les appels dont la
// source 0 est la projection de sortie (nom de tenseur "output.weight").
// Aucune recompilation de la bibliotheque : preload + RTLD_NEXT.
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <dlfcn.h>

typedef struct ggml_tensor ggml_tensor;
typedef struct ggml_context ggml_context;
typedef ggml_tensor * (*mul_mat_fn)(ggml_context *, ggml_tensor *, ggml_tensor *);

static mul_mat_fn real_mul_mat = NULL;
static unsigned long long n_total = 0;
static unsigned long long n_output = 0;

static const char * tname(ggml_tensor * t);

ggml_tensor * ggml_mul_mat(ggml_context * ctx, ggml_tensor * a, ggml_tensor * b) {
    if (real_mul_mat == NULL) {
        real_mul_mat = (mul_mat_fn) dlsym(RTLD_NEXT, "ggml_mul_mat");
    }
    n_total++;
    const char * na = tname(a);
    if (na != NULL && strcmp(na, "output.weight") == 0) {
        n_output++;
    }
    return real_mul_mat(ctx, a, b);
}

// ggml_get_name est exporte par ggml-base : on le resout paresseusement.
typedef const char * (*get_name_fn)(const ggml_tensor *);
static const char * tname(ggml_tensor * t) {
    static get_name_fn f = NULL;
    if (f == NULL) {
        f = (get_name_fn) dlsym(RTLD_NEXT, "ggml_get_name");
        if (f == NULL) { return NULL; }
    }
    return f(t);
}

__attribute__((destructor)) static void report(void) {
    fprintf(stderr, "E5E_PROBE_MUL_MAT_TOTAL=%llu\nE5E_PROBE_MUL_MAT_OUTPUT_WEIGHT=%llu\n",
            n_total, n_output);
}
