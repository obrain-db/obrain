#!/usr/bin/env python3
"""ENGINE-007B-F1A-I2 — greffe du POINT D EXTRACTION dans llama-context.cpp.

Fenetre de duree de vie, prouvee statiquement (LIFETIME_CONTROL_FLOW_PROOF.txt) :

    COMPUTE (graph_compute_async, llama-context.cpp:2942)
      < SYNCHRONIZE + EXTRACTION  (insere ici, apres l appel de graph_compute)
        < INVALIDATION (prochaine soumission de graphe, ou sched_reserve)

On synchronise explicitement : le planificateur peut reutiliser ses buffers a la
prochaine soumission (commentaire source a llama-context.cpp:1708).

Inerte sans OBRAIN_F1A_CAPTURE=1. Une seule ancre, exactement une occurrence.
"""
import sys

ANCHOR = "    const auto status = graph_compute(res->get_gf(), ubatch.n_tokens > 1);"

REPL = ANCHOR + "\n" + """    // [F1A-CAPTURE] fenetre de duree de vie : synchronisation puis extraction, AVANT
    // toute reutilisation possible des buffers du planificateur par le tour suivant.
    if (f1a_capture_active(f1a_capture_get_ctx())) {
        f1a_capture_end_epoch(f1a_capture_get_ctx(),
                              ggml_backend_sched_get_backend(sched.get(), 0));
    }"""

INC_ANCHOR = '#include "llama-context.h"'
INC_REPL   = '#include "llama-context.h"\n#include "f1a-capture.h"   // [F1A-CAPTURE]'


def main():
    if len(sys.argv) < 2:
        raise SystemExit("usage: apply-f1a-context-graft.py <chemin/llama-context.cpp>")
    path = sys.argv[1]
    text = open(path, encoding="utf-8").read()
    if "f1a_capture_end_epoch" in text:
        raise SystemExit("deja applique")
    for anchor, name in ((INC_ANCHOR, "include"), (ANCHOR, "appel graph_compute")):
        n = text.count(anchor)
        if n != 1:
            raise SystemExit("ANCRE '%s' : %d occurrence(s), attendu 1 -- rien n'est ecrit" % (name, n))
    text = text.replace(INC_ANCHOR, INC_REPL, 1)
    text = text.replace(ANCHOR, REPL, 1)
    open(path, "w", encoding="utf-8").write(text)
    print("greffe contexte appliquee : %s" % path)


if __name__ == "__main__":
    main()
