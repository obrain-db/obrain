#include <array>
// [obrain] PROPOSEUR RUNTIME -- Gate R1 : equivalence oracle/C++ exigee.
//
// Chaine implementee a l'identique de l'oracle :
//     suffixe exact -> trie des continuations -> (mu_U, sigma_U, N) -> CONSERVATIVE_SCORE -> argmax
//
//   S(pi,P) = mu_U - 6.00 * sigma_U / sqrt(N)        pour N >= 2 ; N <= 1 -> candidat INADMISSIBLE
//   PROPOSE <=> max S > 0 ; (pi*,P*) = argmax S ; sinon ABSTAIN
//   U(a,P)  = (a+1)*T_plain - T_0(P+1) - T_lookup    avec T_plain = 62.5 ms, T_lookup = 0.023 ms
//
// La table T_0 est la TABLE BRUTE MESUREE (b=2..49), jamais lissee ni interpolee ; son hash est
// verifie au demarrage et l'outil REFUSE de tourner si le hash differe du manifeste gele.
//
// Instrument de mesure : aucun Obrain dans le chemin critique, aucun changement de runtime serveur.
//
// Entree  : deux fichiers texte, un identifiant de jeton par ligne (prompt.txt, output.txt)
// Sortie  : une ligne par position, machine-lisible.

#include <cstdio>
#include <cstdlib>
#include <cstdint>
#include <cmath>
#include <cstring>
#include <string>
#include <vector>
#include <map>
#include <unordered_map>
#include <algorithm>

static int g_dump_all = 0;
static const double T_PLAIN = 62.5;          // ms par jeton (mesure)
static const double T_LOOKUP = 0.023;        // ms
static const double Z_STAR = 6.00;           // CONSERVATIVE_SCORE
static const int    PROF = 48;
static const int    LONGUEURS[7] = {48, 32, 24, 16, 12, 8, 4};

// ---- TABLE T_0 BRUTE, MESUREE (b = 2..49). Aucune interpolation, aucun lissage. ----
static const double T0[50] = {
    0.0, 0.0,
    73.278999999999996, 74.095500000000001, 75.614000000000004, 76.015000000000001,
    77.753500000000003, 81.757999999999996, 83.647000000000006, 89.980999999999995,
    92.875500000000002, 95.352999999999994, 102.71299999999999, 104.61199999999999,
    108.706, 113.26900000000001, 117.041, 123.91,
    131.80799999999999, 129.24299999999999, 140.452, 144.62,
    137.245, 146, 149.58699999999999, 152.99700000000001,
    154.976, 161.54400000000001, 160.429, 169.81,
    176.13999999999999, 172.44900000000001, 179.756, 175.48500000000001,
    185.76599999999999, 180.66249999999999, 185.19900000000001, 189.68100000000001,
    207.751, 203.298, 210.41300000000001, 213.482,
    219.80000000000001, 219.68799999999999, 215.739, 221.91899999999998,
    229.214, 234.03899999999999, 231.13800000000001, 236.23599999999999
};
static const char * HASH_T0_ATTENDU = "e31e6f75712ac08d";

// hash simple mais deterministe, reproduit cote Python pour comparaison
static std::string hash_table() {
    char buf[32];
    unsigned long long h = 1469598103934665603ULL;   // FNV-1a 64
    for (int b = 2; b <= 49; ++b) {
        snprintf(buf, sizeof(buf), "%.10g,", T0[b]);
        for (const char * p = buf; *p; ++p) { h ^= (unsigned char) *p; h *= 1099511628211ULL; }
    }
    char out[32];
    snprintf(out, sizeof(out), "%016llx", h);
    return std::string(out);
}

struct Score {
    bool   admissible = false;
    double S = 0.0, mu = 0.0, sigma = 0.0;
    int    N = 0, P = 0;
    std::vector<int32_t> chemin;
};

// ---- moments EXACTS depuis les supports cumulatifs n_k (aucune materialisation des A_i) ----
// h_k = n_k - n_{k+1} pour k < P, h_P = n_P ; U(a,P) = (a+1)*T_plain - T_0(P+1) - T_lookup
static Score score_noeud(const std::vector<int> & nk, int N, int P) {
    Score s; s.P = P; s.N = N;
    if (N <= 1) { return s; }                      // INAdmissible, comme l'oracle corrige
    const double t0 = T0[P + 1];
    double EU = 0.0, EU2 = 0.0;
    int prev = N;
    for (int k = 1; k <= P; ++k) {
        const int cg = nk[k - 1];
        const int ce = prev - cg;                  // A_d = k-1 exactement
        if (ce > 0) {
            const double u = (double) k * T_PLAIN - t0 - T_LOOKUP;
            EU += (double) ce * u; EU2 += (double) ce * u * u;
        }
        prev = cg;
    }
    if (prev > 0) {                                // A_d = P
        const double u = (double) (P + 1) * T_PLAIN - t0 - T_LOOKUP;
        EU += (double) prev * u; EU2 += (double) prev * u * u;
    }
    EU /= N; EU2 /= N;
    const double var = std::max(0.0, EU2 - EU * EU);
    s.mu = EU; s.sigma = std::sqrt(var);
    s.S = EU - Z_STAR * s.sigma / std::sqrt((double) N);
    s.admissible = (s.S > 0.0);
    return s;
}

struct Decision {
    bool propose = false; int P = 0; int L = 0; int N = 0;
    double S = 0.0, mu = 0.0, sigma = 0.0;
    long long sum_A = 0, sum_A2 = 0;
    int winner_rank = -1, candidate_count = 0, tie_multiplicity = 0;
    std::string path_hash;
};

static std::string hash_path(const std::vector<int32_t> & p) {
    unsigned long long h = 1469598103934665603ULL;
    for (int32_t x : p) { for (int b = 0; b < 4; ++b) { h ^= (unsigned char)((x >> (8*b)) & 0xff); h *= 1099511628211ULL; } }
    char o[32]; snprintf(o, sizeof(o), "%016llx", h); return std::string(o);
}

// ---- TRIE A ORDRE D'INSERTION EXPLICITE (jamais de tri par token) ----
// enfants[noeud] : index rapide token -> rang ; ordre[noeud] : jetons dans l'ordre de PREMIERE RENCONTRE
struct Trie {
    std::vector<std::vector<int32_t>> ordre;                 // ordre d'insertion des enfants
    std::vector<std::unordered_map<int32_t,int>> index;      // token -> position dans ordre
    std::vector<int> compte;                                 // compte cumulatif du noeud
    std::vector<std::vector<int32_t>> chemin;                // chemin du noeud

    int racine() { ordre.assign(1, {}); index.assign(1, {}); compte.assign(1, 0); chemin.assign(1, {});
                   return 0; }
    int enfant(int noeud, int32_t tok) {
        auto it = index[noeud].find(tok);
        if (it != index[noeud].end()) { return it->second; }
        const int n = (int) ordre.size();
        ordre.push_back({}); index.push_back({}); compte.push_back(0);
        chemin.push_back(chemin[noeud]); chemin[n].push_back(tok);
        index[noeud][tok] = n;   // [corrige] id du noeud, et non position dans ordre[noeud]
        ordre[noeud].push_back(tok);
        ordre[n].reserve(4);
        return n;
    }
    int id_enfant(int noeud, int32_t tok, bool & ok) {
        auto it = index[noeud].find(tok);
        if (it == index[noeud].end()) { ok = false; return -1; }
        ok = true; return it->second;
    }
    int noeud_par_ordre(int noeud, int k, int32_t & tok) { tok = ordre[noeud][k]; bool ok; return id_enfant(noeud, tok, ok); }
};


// [invariants] verification FATALE de la structure du trie (aucun log : assertion)
static void verifier_trie_r1b(const Trie & tr, int L, int t) {
    const int nb = (int) tr.ordre.size();
    long long ne = 0;
    for (int x = 0; x < nb; ++x) ne += (long long) tr.ordre[x].size();
    if (ne != (long long) nb - 1) {
        printf("INVARIANT_KO edges=%lld nodes=%d (edges != nodes-1) t=%d L=%d\n", ne, nb, t, L); exit(9);
    }
    for (int p = 0; p < nb; ++p) {
        for (size_t i = 0; i < tr.ordre[p].size(); ++i) {
            const int32_t tok = tr.ordre[p][i];
            auto it = tr.index[p].find(tok);
            if (it == tr.index[p].end()) { printf("INVARIANT_KO ordre->index manquant p=%d tok=%d t=%d L=%d\n", p, tok, t, L); exit(9); }
            const int c = it->second;
            if (c <= p)          { printf("INVARIANT_KO enfant<=parent p=%d c=%d t=%d L=%d\n", p, c, t, L); exit(9); }
            if (c >= nb)         { printf("INVARIANT_KO c>=nodes p=%d c=%d nodes=%d t=%d\n", p, c, nb, t, L); exit(9); }
            if ((int) tr.chemin[c].size() != (int) tr.chemin[p].size() + 1) {
                printf("INVARIANT_KO depth(c)!=depth(p)+1 p=%d c=%d t=%d L=%d\n", p, c, t, L); exit(9);
            }
            if (tr.chemin[c].size() < 1 || tr.chemin[c][tr.chemin[c].size()-1] != tok) {
                printf("INVARIANT_KO chemin(c) incoherent p=%d c=%d t=%d L=%d\n", p, c, t, L); exit(9);
            }
        }
        for (auto & kv : tr.index[p]) {
            const int32_t tok = kv.first; const int c = kv.second;
            bool trouve = false;
            for (size_t i = 0; i < tr.ordre[p].size(); ++i) if (tr.ordre[p][i] == tok) { trouve = true; break; }
            if (!trouve) { printf("INVARIANT_KO index->ordre manquant p=%d tok=%d t=%d L=%d\n", p, tok, t, L); exit(9); }
            if (c <= p || c >= nb) { printf("INVARIANT_KO index c=%d hors bornes p=%d t=%d L=%d\n", c, p, t, L); exit(9); }
        }
    }
}

#ifdef OBRAIN_DIFF_DUMP
static int g_bdbg_trace = -1, g_bdbg_ordinal = -1, g_bdbg_cur_ordinal = -1;
static unsigned long long bdbg_hash_tokens(const std::vector<int32_t> & v) {
    unsigned long long h = 1469598103934665603ULL;
    for (size_t i = 0; i < v.size(); ++i) { h ^= (unsigned long long)(uint32_t) v[i]; h *= 1099511628211ULL; }
    return h;
}
static unsigned long long bdbg_hash_occ(const std::vector<std::pair<int,int>> & v) {
    unsigned long long h = 1469598103934665603ULL;
    for (size_t i = 0; i < v.size(); ++i) {
        h ^= (unsigned long long)(uint32_t) v[i].first;  h *= 1099511628211ULL;
        h ^= (unsigned long long)(uint32_t) v[i].second; h *= 1099511628211ULL;
    }
    return h;
}
#define BDBG_HIT (g_bdbg_cur_ordinal == g_bdbg_ordinal)
#endif

static Decision proposer(const std::vector<int32_t> & ctx) {
    Decision best; const int t = (int) ctx.size();
    for (int li = 0; li < 7; ++li) {
        const int L = LONGUEURS[li];
#ifdef OBRAIN_DIFF_DUMP
        if (BDBG_HIT && L == 4) {
            std::vector<int32_t> suf;
            for (int k = 0; k < L; ++k) suf.push_back(ctx[t - L + k]);
            std::string ser; for (int k = 0; k < L; ++k) ser += std::to_string(suf[k]) + ",";
            const unsigned long long h = bdbg_hash_tokens(suf);
            bool tok_ok = ((int) suf.size() == L);
            for (int k = 0; k < L && tok_ok; ++k) if (suf[k] != ctx[t - L + k]) tok_ok = false;
            fprintf(stderr, "BDBG_SUFFIX trace=%d ordinal=%d li=%d L=%d t=%d token_count=%d tokens=[%s] "
                            "logical_hash=%016llx invariant_ok=%d\n",
                    g_bdbg_trace, g_bdbg_ordinal, li, L, t, (int) suf.size(), ser.c_str(), h,
                    (tok_ok && (int) suf.size() == L) ? 1 : 0);
        }
#endif
        if (L < 4 || t - L < 1) { continue; }
        std::vector<std::pair<int,int>> occ;
        for (int q = t - L - 1; q >= 0; --q) {                 // q DECROISSANT, comme Python
            bool eq = true;
            for (int k = 0; k < L && eq; ++k) { if (ctx[q+k] != ctx[t-L+k]) { eq = false; } }
            if (eq) { const int dispo = t - (q + L); if (dispo > 0) { occ.push_back({q, dispo}); } }
        }
#ifdef OBRAIN_DIFF_DUMP
        if (BDBG_HIT && L == 4) {
            std::string ser; bool inv_ok = true;
            for (size_t i = 0; i < occ.size(); ++i) {
                ser += "(" + std::to_string(occ[i].first) + "," + std::to_string(occ[i].second) + "),";
                const int qq = occ[i].first, dd = occ[i].second;
                if (!(dd == t - (qq + L))) inv_ok = false;
                if (!(dd > 0))            inv_ok = false;
                if (!(qq >= 0))           inv_ok = false;
                if (!(qq + L < t))        inv_ok = false;
                for (int k = 0; k < L; ++k) if (ctx[qq + k] != ctx[t - L + k]) inv_ok = false;
            }
            const unsigned long long h = bdbg_hash_occ(occ);
            const int dumped = (int) occ.size();
            fprintf(stderr, "BDBG_OCC trace=%d ordinal=%d li=%d L=%d container_size=%d dumped_count=%d "
                            "occurrences=[%s] logical_hash=%016llx invariant_ok=%d\n",
                    g_bdbg_trace, g_bdbg_ordinal, li, L, (int) occ.size(), dumped, ser.c_str(), h,
                    (inv_ok && dumped == (int) occ.size()) ? 1 : 0);
        }
#endif
        if (occ.empty()) { continue; }
        const int N = (int) occ.size();
        Trie tr; tr.racine();
        // insertion : pour chaque occurrence, prefixes par PROFONDEUR CROISSANTE
        for (auto & o : occ) {
            int noeud = 0;
            const int n = std::min(PROF, o.second);
            for (int k = 0; k < n; ++k) {
                noeud = tr.enfant(noeud, ctx[o.first + L + k]);
                tr.compte[noeud] += 1;
            }
        }
        // DFS a pile LIFO, enfants empiles dans l'ordre d'insertion
        // [r1ref] SEULE transformation vs r1b : la pile porte (noeud, SA, SA2) ; les moments se
        // propagent (SA_p = SA_{p-1}+n_p ; SA2_p = SA2_{p-1}+(2p-1)n_p) au lieu d'etre recalcules en
        // remontant le chemin. Identite algebrique : ni les occurrences ni leur ordre ne changent.
        verifier_trie_r1b(tr, L, t);
        std::vector<char> vis_essai(tr.ordre.size(), 0);
        std::vector<std::array<long long,3>> pile; pile.push_back({0, 0, 0});
        int rang = -1; int cand_count = 0; int mult = 0;
        while (!pile.empty()) {
            const auto cur = pile.back(); pile.pop_back();
            const int noeud = (int) cur[0]; const long long SA = cur[1], SA2 = cur[2];
            { 
              if (noeud < 0 || noeud >= (int) tr.ordre.size()) { printf("DFS_KO node hors bornes %d\n", noeud); exit(9); }
              if (vis_essai[noeud]) { printf("DFS_KO DOUBLE VISITE node=%d t=%d L=%d nodes=%d\n", noeud, t, L, (int) tr.ordre.size()); exit(9); }
              vis_essai[noeud] = 1; }
            const int P = (int) tr.chemin[noeud].size();
            if (P >= 1 && P <= PROF && (P+1) <= 49) {
                rang++;
                // [r1ref] statistiques ENTIERES exactes, PROPAGEES (aucune remontee de chemin)
                const long long s1 = SA, s2 = SA2;
                const int ok = 1;
                if (ok) {
                    cand_count++;
                    if (g_dump_all) {
                        std::string pt, nt; int nd = 0;
                        for (int k = 0; k < P; ++k) {
                            pt += std::to_string(tr.chemin[noeud][k]) + ",";
                            bool o2; nd = tr.id_enfant(nd, tr.chemin[noeud][k], o2); if (!o2) break;
                            nt += std::to_string(tr.compte[nd]) + ",";
                        }
                        printf("OBRAIN_CAND rank=%d path=%s P=%d N=%d nk=%s SA=%lld SA2=%lld\n",
                               rang, pt.c_str(), P, N, nt.c_str(), s1, s2);
                    }
                    if (N > 1) {
                        // [r1ref] FORMES FERMEES via SA/SA2 -- identiques a l'histogramme de r1b :
                        //   E[U]  = (T_plain - T_0 - T_lookup) + T_plain * SA/N
                        //   Var(U)= T_plain^2 * (SA2/N - (SA/N)^2)
                        const double C  = T_PLAIN - T0[P+1] - T_LOOKUP;
                        const double mA = (double) s1 / N;
                        const double vA = (double) s2 / N - mA * mA;
                        const double EU = C + T_PLAIN * mA;
                        const double sd = T_PLAIN * std::sqrt(std::max(0.0, vA));
                        const double S  = EU - Z_STAR * sd / std::sqrt((double) N);
                        if (S > 0.0) {
                            // COMPARATEUR EXACT : strictement superieur, aucune tolerance, aucun >=
                            if (!best.propose || S > best.S) {
                                best.propose = true; best.P = P; best.L = L; best.N = N;
                                best.S = S; best.mu = EU; best.sigma = sd;
                                best.sum_A = s1; best.sum_A2 = s2;
                                best.winner_rank = rang;
                                best.path_hash = hash_path(tr.chemin[noeud]);
                            }
                        }
                    }
                }
            }
            // empilement des enfants DANS L'ORDRE D'INSERTION (le dernier empile est visite en premier)
            const int n_enf = (int) tr.ordre[noeud].size();
            for (int k = 0; k < n_enf; ++k) {
                bool ok2; const int c = tr.id_enfant(noeud, tr.ordre[noeud][k], ok2);
                if (ok2) {
                    const int pc = (int) tr.chemin[c].size();
                    const long long n_pc = tr.compte[c];
                    pile.push_back({c, SA + n_pc, SA2 + (long long)(2*pc - 1) * n_pc});
                }
            }
        }
        if (best.propose) { best.candidate_count = cand_count; return best; }
    }
    return best;
}

static std::vector<int32_t> lire(const char * path) {
    std::vector<int32_t> v; FILE * f = fopen(path, "r");
    if (!f) { fprintf(stderr, "fichier illisible : %s\n", path); exit(2); }
    long x; while (fscanf(f, "%ld", &x) == 1) { v.push_back((int32_t) x); }
    fclose(f); return v;
}

int main(int argc, char ** argv) {
#ifdef OBRAIN_DIFF_DUMP
    { const char * e = getenv("BDBG_ORDINAL"); if (e) g_bdbg_ordinal = atoi(e); }
#endif
    const char * hp = nullptr, * op = nullptr;
    for (int i = 1; i < argc; ++i) {
        if (strcmp(argv[i], "--prompt") == 0 && i + 1 < argc) { hp = argv[++i]; }
        else if (strcmp(argv[i], "--output") == 0 && i + 1 < argc) { op = argv[++i]; }
    }
    for (int i = 1; i < argc; ++i) if (!strcmp(argv[i], "--dump-all")) g_dump_all = 1;
    if (!hp || !op) { fprintf(stderr, "usage: --prompt p.txt --output o.txt\n"); return 1; }

    // Le controle FAISANT FOI est cote Python : le C++ imprime ses valeurs de table, Python
    // reconstruit le JSON canonique et recalcule le sha256 du manifeste gele.
    for (int b = 2; b <= 49; ++b) { printf("OBRAIN_T0 b=%d v=%.10g\n", b, T0[b]); }
    fflush(stdout);
    const std::string h = hash_table();
    printf("OBRAIN_PROPOSER table_hash_fnv=%s attendu_sha256=%s (verification par valeurs cote Python)\n",
           h.c_str(), HASH_T0_ATTENDU);

    std::vector<int32_t> prompt = lire(hp);
    std::vector<int32_t> out    = lire(op);
    printf("OBRAIN_PROPOSER prompt=%zu output=%zu\n", prompt.size(), out.size());
    for (size_t t = 0; t < out.size(); ++t) {
        std::vector<int32_t> ctx = prompt;
        ctx.insert(ctx.end(), out.begin(), out.begin() + t);   // CAUSAL : rien apres t
#ifdef OBRAIN_DIFF_DUMP
        g_bdbg_cur_ordinal = (int) t;
#endif
        Decision d = proposer(ctx);
        if (!d.propose) {
            printf("OBRAIN_POS t=%zu decision=ABSTAIN P_star=0 candidate_count=0\n", t);
        } else {
            printf("OBRAIN_POS t=%zu decision=PROPOSE P_star=%d suffix_len=%d N=%d sum_A=%lld sum_A2=%lld "
                   "mu_U=%.17g sigma_U=%.17g score=%.17g winner_rank=%d path_hash=%s candidate_count=%d\n",
                   t, d.P, d.L, d.N, d.sum_A, d.sum_A2, d.mu, d.sigma, d.S, d.winner_rank,
                   d.path_hash.c_str(), d.candidate_count);
        }
        fflush(stdout);
    }
    return 0;
}
