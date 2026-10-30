#!/usr/bin/env python3
"""ENGINE-007B-F1A-Q0-R4 — R4-a : publier les compteurs independamment de la capture.

Probleme : les compteurs sont incrementes en permanence, mais leur PUBLICATION est
declenchee par f1a_capture_end_epoch(), qui n'est jamais appelee quand la capture est
desactivee. On ne peut donc pas prouver l'inertie par les compteurs.

Correctif : un point de publication separe du cycle de vie de la capture, appele par le
harnais a la fin du test, quelle que soit l'activation. Il ne cree aucun contexte F1A,
aucun tenseur diagnostic, aucune allocation.

Ajoute aussi la lecture du compteur de DUP ajoutes par F1A (R4-a : verifier l'absence de
noeuds DUP diagnostiques quand la capture est off).
"""
import sys

H = "/home/triviere/projects/llama-obrain-cuda-f1a/src/f1a-capture.h"
C = "/home/triviere/projects/llama-obrain-cuda-f1a/src/f1a-capture.cpp"
E = "/home/triviere/projects/llama-obrain-cuda-f1a/tools/f1a-eval.cpp"

# ---- en-tete : deux nouvelles declarations
H_OLD = "void f1a_dump_submitted_snapshots();"
H_NEW = ("""// R4-a : PUBLIER les compteurs SANS creer de contexte de capture.
// Appelee par le harnais a la fin du test, capture active ou non. Aucune allocation,
// aucun tenseur, aucune ecriture : uniquement la lecture des compteurs.
void f1a_publish_counters_standalone(void);

// R4-a : nombre de noeuds DUP ajoutes par la greffe F1A. Doit rester 0 capture off.
unsigned long long f1a_dup_nodes_added(void);

void f1a_dump_submitted_snapshots();""")

# ---- source : compteur de dup + publication autonome
C_OLD = "unsigned long long f1a_compute_submission_count() { return f1a_ctr_submission; }"
C_NEW = C_OLD + """

// R4-a : compte les DUP introduits par la greffe. Incremente a chaque greffe active.
unsigned long long f1a_ctr_dup_added = 0;

unsigned long long f1a_dup_nodes_added(void) { return f1a_ctr_dup_added; }

static void f1a_print_counters(const char * tag) {
    fprintf(stderr, "[f1a-ctr%s] ctx_active=%llu eligible_enter=%llu note=%llu extract_call=%llu payload_write=%llu dup_added=%llu\\n",
            tag, f1a_ctr_ctx_active, f1a_ctr_eligible_enter, f1a_ctr_note,
            f1a_ctr_extract_call, f1a_ctr_payload_write, f1a_ctr_dup_added);
}

void f1a_publish_counters_standalone(void) {
    // Aucun contexte cree, aucun tenseur, aucune E/S : lecture pure.
    f1a_print_counters("_standalone");
}"""

# chaque appel de capture reelle incremente le compteur de DUP
C_DUP_OLD = "        ggml_tensor * const f1a_cur_cap = ggml_dup(ctx0, cur);"
# (cette ligne est dans llama-graph.cpp, pas ici : on compte depuis note())
C_NOTE_OLD = "    if (cc == nullptr) return;\n    f1a_ctr_note++;"
C_NOTE_NEW = "    if (cc == nullptr) return;\n    f1a_ctr_note++;\n    f1a_ctr_dup_added += 2;   // R4-a : cur_capture + mm_capture"

# ---- harnais : point de publication a la fin, inconditionnel
E_OLD = "    llama_free(ctx);"
E_NEW = ("""    // [F1A-CAPTURE] R4-a : publication des compteurs, INCONDITIONNELLE.
    // Appelee meme quand la capture est desactivee : c est ce qui rend l inertie
    // du chemin prouvable autrement que par l absence de fichiers.
    f1a_publish_counters_standalone();

    llama_free(ctx);""")
E_INC_OLD = '#include "llama.h"'
E_INC_NEW = '#include "llama.h"\n#include "f1a-capture.h"   // [F1A-CAPTURE] R4-a'


def main():
    for path, old, new, name in (
        (H, H_OLD, H_NEW, "en-tete"),
        (C, C_OLD, C_NEW, "compteur dup + publication"),
        (C, C_NOTE_OLD, C_NOTE_NEW, "increment dup a chaque note"),
        (E, E_INC_OLD, E_INC_NEW, "include harnais"),
        (E, E_OLD, E_NEW, "appel de publication"),
    ):
        s = open(path, encoding="utf-8").read()
        n = s.count(old)
        if n != 1:
            raise SystemExit("[%s] ancre trouvee %d fois, attendu 1 -- rien n'est ecrit" % (name, n))
        open(path, "w", encoding="utf-8").write(s.replace(old, new, 1))
        print("[%s] ok" % name)
    print("R4-a applique")


if __name__ == "__main__":
    main()
