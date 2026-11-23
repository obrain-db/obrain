// tools/server/obrain-diff-harness.cpp -- HARNESS DE GATE DIFFERENTIELLE
//
// But : etablir  Proposer_server == Proposer_R1_certified  sur les 3840 positions gelees.
//
// Ce harness INCLUT directement l'en-tete du serveur et appelle LE MEME propose().
// Il ne reimplemente AUCUNE politique, et n'applique AUCUN element runtime :
//   pas de verify_capacity, pas de remaining, pas de P_eff, pas de MTP,
//   pas de batch target, pas de rollback, pas de slot serveur.
//
// Il dumps, par position, avant tout clipping runtime :
//   trace_id ordinal decision P_star path_length path_ids[] N SA SA2 mu sigma score
//
// Alimentation : prompt + sortie gelee (meme corpus que la fermeture R1), position par position,
// exactement comme le standalone certifie construit son etat.

#include "obrain-proposer.h"

int g_diff_trace = -1;
int g_diff_ordinal = -1;
int g_diff_cur_trace = -1;
int g_diff_cur_ordinal = -1;

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

static std::vector<int32_t> lire_tokens(const char * path) {
    std::vector<int32_t> v;
    FILE * f = fopen(path, "r");
    if (!f) { fprintf(stderr, "HARNESS_KO : impossible d'ouvrir %s\n", path); exit(2); }
    long x;
    while (fscanf(f, "%ld", &x) == 1) v.push_back((int32_t) x);
    fclose(f);
    return v;
}

int main(int argc, char ** argv) {
    const char * hp = nullptr, * op = nullptr;
    int trace_id = -1;
    for (int i = 1; i < argc; ++i) {
        if (!strcmp(argv[i], "--prompt") && i + 1 < argc) hp = argv[++i];
        else if (!strcmp(argv[i], "--output") && i + 1 < argc) op = argv[++i];
        else if (!strcmp(argv[i], "--trace-id") && i + 1 < argc) trace_id = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--diff-trace") && i + 1 < argc) g_diff_trace = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--diff-ordinal") && i + 1 < argc) g_diff_ordinal = atoi(argv[++i]);
    }
    if (!hp || !op) { fprintf(stderr, "usage: %s --prompt P --output O --trace-id N\n", argv[0]); return 1; }

    const std::vector<int32_t> prompt = lire_tokens(hp);
    const std::vector<int32_t> out    = lire_tokens(op);

    // etat du proposeur : prompt initialise UNE fois, puis chaque jeton COMMITTED
    obrain_proposer_state st;
    st.init(prompt, 0, 0);

    for (int ti = 0; ti < (int) out.size(); ++ti) {
        // on appelle le meme propose() que le serveur, avec la capacite du STANDALONE
        // (borne haute) : la gate porte sur la POLITIQUE BRUTE, pas sur la capacite serveur.
        g_diff_cur_trace = trace_id; g_diff_cur_ordinal = ti;
        const obrain_decision d = st.propose(OBRAIN_B_MAX);
        const int P = d.propose ? d.P : 0;
        std::string ids;
        if (d.propose) {
            for (size_t k = 0; k < d.chemin.size(); ++k) ids += std::to_string((int) d.chemin[k]) + ",";
        }
        printf("DIFF trace=%d ordinal=%d decision=%d P_star=%d path_length=%d path_ids=[%s] "
               "N=%d SA=%lld SA2=%lld mu=%.17g sigma=%.17g score=%.17g\n",
               trace_id, ti, d.propose ? 1 : 0, P, (int) d.chemin.size(), ids.c_str(),
               d.N, d.sum_A, d.sum_A2, d.mu, d.sigma, d.S);
        // la position courante entre dans l'etat APRES la decision (causalite identique au standalone)
        st.update({ out[ti] });
    }
    return 0;
}
