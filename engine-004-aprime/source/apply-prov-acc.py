#!/usr/bin/env python3
# ENGINE-005D : branche l'accumulation par couche + les signatures MUL_MAT sur l'instrument.
# Doit etre applique APRES apply-layer-timing.py.
# Usage : python3 apply-prov-acc.py <ggml-cuda.cu>
import sys, os

ANCHOR = """                for (int iu : lt_used) {
                    float t = 0.0f;
                    CUDA_CHECK(cudaEventElapsedTime(&t, lt_a[iu], lt_b[iu]));
                    if (t > lt_ms[iu]) { lt_ms[iu] = t; }
                }"""

REPL = """                for (int iu : lt_used) {
                    float t = 0.0f;
                    CUDA_CHECK(cudaEventElapsedTime(&t, lt_a[iu], lt_b[iu]));
                    if (t > lt_ms[iu]) { lt_ms[iu] = t; }
                    // [ENGINE-005D] accumulation par couche (provenance) + signature MUL_MAT
                    if (ggml_prov_enabled()) {
                        ggml_prov_acc(lt_layer[iu], lt_op[iu], (double) t);
                        if (lt_op[iu] != nullptr && strcmp(lt_op[iu], "MUL_MAT") == 0) {
                            ggml_tensor * nd = cgraph->nodes[iu];
                            const ggml_tensor * w = (nd->src[0] != nullptr) ? nd->src[0] : nullptr;
                            const ggml_tensor * x = (nd->src[1] != nullptr) ? nd->src[1] : nullptr;
                            ggml_prov_record_matmul(lt_layer[iu],
                                                    (const void *) w,
                                                    w ? ggml_get_name(w) : "?",
                                                    w ? ggml_type_name(w->type) : "?",
                                                    x ? (long long) x->ne[0] : 0,
                                                    x ? (long long) x->ne[1] : 0,
                                                    w ? (long long) w->ne[0] : 0,
                                                    (double) t);
                        }
                    }
                }"""

def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: apply-prov-acc.py <ggml-cuda.cu>")
    path = sys.argv[1]
    text = open(path, encoding="utf-8").read()
    if "[ENGINE-005D] accumulation" in text:
        raise SystemExit("DEJA PATCHE")
    n = text.count(ANCHOR)
    if n != 1:
        raise SystemExit("ANCRE INTROUVABLE OU AMBIGUE : %d" % n)
    text = text.replace(ANCHOR, REPL, 1)
    open(path + ".tmp", "w", encoding="utf-8").write(text)
    os.replace(path + ".tmp", path)
    print("ECRIT %s" % path)

if __name__ == "__main__":
    main()
