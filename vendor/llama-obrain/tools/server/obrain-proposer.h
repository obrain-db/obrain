// obrain-proposer.h -- ETAT DU PROPOSEUR CERTIFIE, branche dans un slot serveur.
//
// CONTRAT GELE (§77) :
//   State_0 = Init(prompt)                UNE SEULE FOIS par requete logique
//   DECIDE  -> draft = Propose(State_t)   site 1 : NE relit JAMAIS slot.prompt.tokens
//   COMMIT  accepted[0:C]
//   UPDATE  -> Update(State_t, accepted)  site 2 : SEULS les jetons COMMITTED
//
// Le corps de propose() est le code CERTIFIE, EXTRAIT VERBATIM de tests/obrain-traj.cpp
// (etapes 4-6 de la reprise R3 : gate dense<->trajectory a 0 mismatch).
// La table T0 est GENERE depuis son artefact canonique et son hash a ete verifie a la
// generation : e31e6f75712ac08d. AUCUNE constante physique recopiee a la main.
#pragma once

#include "obrain-certified-config.h"   // [CONFIG CANONIQUE] parametres geles GENERES

#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <string>
#include <unordered_map>
#include <vector>

// [CONFIG CANONIQUE] AUCUNE COPIE LOCALE des parametres geles : ils viennent du header GENERE
// (sources certifiees -> verification de hash -> generation -> rejouabilite).
// Substitution PURE : aucune ligne algorithmique de propose() n'est touchee.
#ifdef OBRAIN_DIFF_DUMP
extern int g_diff_trace;      // fourni UNIQUEMENT par le harness diagnostique
extern int g_diff_ordinal;
extern int g_diff_cur_trace;
extern int g_diff_cur_ordinal;
#define OBRAIN_DIFF_HIT (g_diff_cur_trace == g_diff_trace && g_diff_cur_ordinal == g_diff_ordinal)
#endif

#define OBRAIN_NL         OBRAIN_POLICY_NL
#define OBRAIN_LONGUEURS  OBRAIN_POLICY_LONGUEURS
#define OBRAIN_PROF       OBRAIN_POLICY_PROF
#define OBRAIN_T_PLAIN    OBRAIN_POLICY_T_PLAIN
#define OBRAIN_T_LOOKUP   OBRAIN_POLICY_T_LOOKUP
#define OBRAIN_Z          OBRAIN_POLICY_Z
#define OBRAIN_T0         OBRAIN_POLICY_T0

static const int OBRAIN_B_MAX = 49;   // borne GPU/noyau (TQ2_G128 <= 49), HORS politique

// [R3-B decouplage] CAPACITE AVAL D'OBRAIN, EXPLICITEMENT CONFIGUREE.
// Elle ne doit PLUS etre heritee du brouillon MTP. Defaut 3 = comportement anterieur (inchange).
static int obrain_verify_capacity_b() {
    static int cap = -1;
    if (cap < 0) {
        const char * e = getenv("OBRAIN_CAPACITY_B");
        cap = e ? atoi(e) : 3;
        if (cap < 2) cap = 2;
        if (cap > OBRAIN_B_MAX) cap = OBRAIN_B_MAX;
    }
    return cap;
}

// [CONFIG CANONIQUE] table T0 fournie par obrain-certified-config.h (OBRAIN_POLICY_T0)
static const char * OBRAIN_T0_HASH = OBRAIN_CFG_T0_CANONICAL_HASH;

static std::string hash_path(const std::vector<int32_t> & p) {
    unsigned long long h = 1469598103934665603ULL;
    for (int32_t x : p) for (int b = 0; b < 4; ++b) { h ^= (uint8_t)((x >> (8*b)) & 0xff); h *= 1099511628211ULL; }
    char o[32]; snprintf(o, sizeof(o), "%016llx", h); return o;
}
struct obrain_decision {
    bool propose = false;
    int P = 0, L = 0, N = 0;
    double mu = 0, sigma = 0, S = 0;
    long long sum_A = 0, sum_A2 = 0;
    int winner_rank = -1, candidate_count = 0;
    std::string path_hash;
    std::vector<int32_t> chemin;
};

// [R3-A4] LEDGER UNIQUE : une ligne par evenement qui compte un token comme SORTIE ou comme COMMIT.
struct obrain_event {
    long long seq;
    int       slot;
    const char * type;      // OUTPUT_NORMAL | OUTPUT_SPEC | COMMIT_NORMAL | COMMIT_SPEC
    int       tok;
    long long out_after, com_after, state;
    int       is_stop, is_eog;
};
static std::vector<obrain_event> g_obrain_ledger;
static long long                 g_obrain_seq = 0;
// [R3-A7] Le pre-Init est indexe PAR CONTEXTE : (slot serveur, generation). Un slot recycle
// entre deux requetes ne partage donc jamais son ledger, et un contexte AUXILIAIRE (slot=-1)
// n'entre jamais dans la comptabilite d'un slot.
static std::unordered_map<int64_t, std::vector<int32_t>> g_pre_init_by_ctx;
static int64_t g_pre_init_ctx_key(int64_t slot, int64_t gen) { return slot * 1000000 + gen; }
// accesseurs publics (la frontiere causale est posee par le serveur, pas par l'en-tete)
static int64_t g_pre_init_ctx_key_public(int64_t slot, int64_t gen) { return g_pre_init_ctx_key(slot, gen); }

static void obrain_ledger_add(int slot, const char * type, int tok,
                              long long out_after, long long com_after, long long state,
                              int is_stop, int is_eog) {
    g_obrain_ledger.push_back({++g_obrain_seq, slot, type, tok, out_after, com_after, state, is_stop, is_eog});
    fprintf(stderr, "OBRAIN_LEDGER seq=%lld slot=%d type=%s tok=%d out_after=%lld com_after=%lld "
                    "state=%lld is_stop=%d is_eog=%d\n",
            g_obrain_seq, slot, type, tok, out_after, com_after, state, is_stop, is_eog);
    fflush(stderr);
}

struct obrain_proposer_state {
    static std::unordered_map<int64_t, std::vector<int32_t>> & g_pre_init_by_ctx_public() { return g_pre_init_by_ctx; }
    std::vector<int32_t> ctx;
    std::vector<std::unordered_map<uint64_t, std::vector<int>>> buckets;
    std::vector<int> insere_par_L;

    // compteurs d'invariants R3-A
    int64_t prompt_tokens_inserted = 0, committed_updates = 0, rejected_draft_updates = 0;
    int64_t normal_commit_tokens = 0, spec_commit_tokens = 0;
    int64_t output_tokens_counted = 0;   // [R3-A4] compte les evenements OUTPUT du ledger
    // [R3-A6] FRONTIERE CAUSALE : History_t = Prompt_at_Init || Committed_post_Init
    int64_t pre_init_output_events = 0;
    int64_t prompt_length_at_init  = 0;
    int32_t prompt_last_token_id   = -1;
    uint64_t prompt_hash_at_init   = 0;
    int      pre_init_in_prompt    = -1;   // 1 = la sequence pre-Init est bien un SUFFIXE du prompt   // exclusivite des deux voies
    int64_t init_count = 0, decision_count = 0, propose_count = 0, abstain_count = 0;
    int64_t sum_P_eff = 0, sum_A_d = 0;
    int64_t request_id = -1, slot_id = -1;
    int64_t generation = 0;               // [R3-A7] incremente a chaque reset() du slot
    int64_t context_id = -1;              // = slot_id * 1000000 + generation
    bool    initialized = false;   // [fix segfault] obrain_commit avant Init -> no-op
    long long occ_scanned = 0, trie_nodes = 0, cands_scored = 0;

    uint64_t hash_suffixe(int q, int L) const {
        uint64_t x = 1469598103934665603ull;
        for (int k = 0; k < L; ++k) { x ^= (uint64_t)(uint32_t) ctx[q + k]; x *= 1099511628211ull; }
        return x;
    }

    void alimenter() {
        const int t = (int) ctx.size();
        for (int li = 0; li < OBRAIN_NL; ++li) {
            const int L = OBRAIN_LONGUEURS[li];
#ifdef OBRAIN_DIFF_DUMP
            if (OBRAIN_DIFF_HIT) {
                fprintf(stderr, "OBRAIN_DIFF_LI trace=%d ordinal=%d li=%d L=%d\n",
                        g_diff_trace, g_diff_ordinal, li, L);
            }
#endif
            while (insere_par_L[li] + L <= t) {
                const int q = insere_par_L[li];
                if (q + L <= (int) ctx.size()) buckets[li][hash_suffixe(q, L)].push_back(q);
                insere_par_L[li]++;
            }
        }
    }

    // ---- PRIMITIVE UNIQUE DE COMMIT : une fois par jeton COMMITTED, quelle que soit la provenance ----
    enum obrain_source { OBRAIN_SRC_NORMAL = 0, OBRAIN_SRC_SPEC = 1 };
    void obrain_commit(const int32_t * toks, int n, int source) {
        // AVANT Init, l'etat n'existe pas encore : les jetons de PROMPT ne sont pas des COMMITTED.
        // Sans ce garde, alimenter() iterait sur insere_par_L/buckets VIDES -> segfault.
        if (!initialized) { return; }
        for (int i = 0; i < n; ++i) {
            ctx.push_back(toks[i]);
            committed_updates++;
            if (source == OBRAIN_SRC_NORMAL) normal_commit_tokens++; else spec_commit_tokens++;
            alimenter();
            obrain_ledger_add((int) slot_id,
                              source == OBRAIN_SRC_NORMAL ? "COMMIT_NORMAL" : "COMMIT_SPEC",
                              (int) toks[i], 0, committed_updates, state_token_count(), -1, -1);
        }
    }
    void obrain_commit(const std::vector<int32_t> & toks, int source) {
        obrain_commit(toks.data(), (int) toks.size(), source);
    }

    void init(const std::vector<int32_t> & prompt, int64_t task_id, int64_t slot, int64_t gen = 0) {
        ctx = prompt;
        buckets.assign(OBRAIN_NL, {}); insere_par_L.assign(OBRAIN_NL, 0);
        prompt_tokens_inserted = (int64_t) prompt.size();
        committed_updates = 0; rejected_draft_updates = 0;
        normal_commit_tokens = 0; spec_commit_tokens = 0;
        output_tokens_counted = 0;
        decision_count = propose_count = abstain_count = 0;
        sum_P_eff = sum_A_d = 0;
        init_count++;
        request_id = task_id; slot_id = slot; generation = gen;
        context_id = g_pre_init_ctx_key(slot, gen);
        // --- frontiere causale : ce qui a ete emis avant Init doit etre DANS le prompt injecte ---
        prompt_length_at_init = (int64_t) ctx.size();
        prompt_last_token_id  = ctx.empty() ? -1 : ctx.back();
        prompt_hash_at_init   = 0;
        for (int32_t tk : ctx) { prompt_hash_at_init ^= (uint64_t)(uint32_t) tk; prompt_hash_at_init *= 1099511628211ull; }
        const std::vector<int32_t> & pre = g_pre_init_by_ctx[context_id];
        pre_init_output_events = (int64_t) pre.size();
        pre_init_in_prompt = 1;
        if (!pre.empty()) {
            const size_t k = pre.size();
            if (k > ctx.size()) { pre_init_in_prompt = 0; }
            else {
                for (size_t i = 0; i < k; ++i) {
                    if (ctx[ctx.size() - k + i] != pre[i]) { pre_init_in_prompt = 0; break; }
                }
            }
        }
        fprintf(stderr, "OBRAIN_R3A init_boundary slot=%lld pre_init_output_events=%lld "
                        "pre_init_seq=[%s] prompt_length_at_init=%lld prompt_last_token_id=%d "
                        "prompt_hash_at_init=%016llx pre_init_is_suffix_of_prompt=%d\n",
                (long long) slot, (long long) pre_init_output_events, [&]{
                    std::string s2; for (int32_t tk : pre) s2 += std::to_string((int) tk) + ",";
                    static std::string s3; s3 = s2; return s3.c_str(); }(),
                (long long) prompt_length_at_init, (int) prompt_last_token_id,
                (unsigned long long) prompt_hash_at_init, pre_init_in_prompt);
        fflush(stderr);
        initialized = true;
        alimenter();
    }

    // ---- SITE 1 : Propose(State_t). Le prompt n'est JAMAIS relu ici. ----
    obrain_decision propose(int n_max) {
        obrain_decision vide;
        if (!initialized) { return vide; }        // pas d'etat -> pas de proposition
        const int t = (int) ctx.size();
        decision_count++;
        occ_scanned = trie_nodes = cands_scored = 0;
            obrain_decision best;
            for (int li = 0; li < OBRAIN_NL; ++li) {
                const int L = OBRAIN_LONGUEURS[li];
                if (t - L < 1) continue;
                std::vector<int> occ;
                auto it = buckets[li].find(hash_suffixe(t - L, L));
                if (it != buckets[li].end()) {
                    for (int q : it->second) {
                        if (q >= t - L) continue;                       // causal
                        bool eq = true;
                        for (int k = 0; k < L && eq; ++k) if (ctx[q+k] != ctx[t-L+k]) eq = false;
                        if (eq && t - (q + L) > 0) occ.push_back(q);
                    }
                }
                if (occ.empty()) continue;
                const int N = (int) occ.size();
                occ_scanned += N;
                // ordre HISTORIQUE : q DECROISSANT
                std::sort(occ.begin(), occ.end(), [](int a, int b){ return a > b; });

                // ---- trie a ordre d'insertion explicite ----
                std::vector<std::vector<int32_t>> ordre(1);            // enfants dans l'ordre de premiere rencontre
                std::vector<std::unordered_map<int32_t,int>> index(1);
                std::vector<int> compte(1, 0);
                std::vector<int> parent(1, -1);
                std::vector<std::vector<int32_t>> chemin(1);
                std::vector<int> nk;
                auto enfant = [&](int noeud, int32_t tok) -> int {
                    auto it2 = index[noeud].find(tok);
                    if (it2 != index[noeud].end()) return it2->second;
                    const int n = (int) ordre.size();
                    ordre.push_back({}); index.push_back({}); compte.push_back(0);
                    parent.push_back(noeud);
                    chemin.push_back(chemin[noeud]); chemin[n].push_back(tok);
                    index[noeud][tok] = n; ordre[noeud].push_back(tok);
                    return n;
                };
                for (int q : occ) {                                    // q decroissant
                    int noeud = 0;
                    const int n = std::min(OBRAIN_PROF, t - (q + L));
                    for (int k = 0; k < n; ++k) { noeud = enfant(noeud, ctx[q + L + k]); compte[noeud] += 1; }
                }
                trie_nodes += (long long) ordre.size();

                // ---- DFS LIFO, enfants empiles dans l'ordre d'insertion, moments propages en O(1) ----
                std::vector<std::array<long long,3>> pile;             // (noeud, SA, SA2)
                pile.push_back({0, 0, 0});
                int rang = -1, ncand = 0;
                while (!pile.empty()) {
                    auto cur = pile.back(); pile.pop_back();
                    const int noeud = (int) cur[0];
                    const long long SA = cur[1], SA2 = cur[2];
                    const int p = (int) chemin[noeud].size();
                    if (p >= 1 && p <= OBRAIN_PROF && (p+1) <= 49) {
                        rang++; ncand++;
#ifdef OBRAIN_DIFF_DUMP
                    if (OBRAIN_DIFF_HIT) {
                        std::string pt, nt; int nd = 0;
                        for (int k = 0; k < p; ++k) {
                            pt += std::to_string((int) chemin[noeud][k]) + ",";
                            auto itd = index[nd].find(chemin[noeud][k]);
                            if (itd == index[nd].end()) { break; }
                            nd = itd->second;
                            nt += std::to_string(compte[nd]) + ",";
                        }
                        fprintf(stderr, "OBRAIN_DIFF_CAND trace=%d ordinal=%d li=%d L=%d rank=%d P=%d "
                                        "path_ids=[%s] N=%d nk=[%s] SA=%lld SA2=%lld\n",
                                g_diff_trace, g_diff_ordinal, li, L, rang, p, pt.c_str(), N, nt.c_str(), SA, SA2);
                    }
#endif
                        if (N > 1) {
                            const double C = OBRAIN_T_PLAIN - OBRAIN_T0[p+1] - OBRAIN_T_LOOKUP;
                            const double mA = (double) SA / N;
                            const double vA = (double) SA2 / N - mA * mA;
                            const double mu = C + OBRAIN_T_PLAIN * mA;
                            const double sd = OBRAIN_T_PLAIN * std::sqrt(std::max(0.0, vA));
                            const double S = mu - OBRAIN_Z * sd / std::sqrt((double) N);
                            if (S > 0.0) {                             // STRICT, aucun >=
                                if (!best.propose || S > best.S) {
                                    best.propose = true; best.P = p; best.L = L; best.N = N;
                                    best.mu = mu; best.sigma = sd; best.S = S;
                                    best.sum_A = SA; best.sum_A2 = SA2;
                                    best.winner_rank = rang;
                                    best.path_hash = hash_path(chemin[noeud]);
                                    best.chemin = chemin[noeud];   // [R2] pour calculer A_d
                                }
                            }
                        }
                    }
                    const int n_enf = (int) ordre[noeud].size();
                    for (int k = 0; k < n_enf; ++k) {                   // ordre d'insertion
                        auto it2 = index[noeud].find(ordre[noeud][k]);
                        if (it2 == index[noeud].end()) continue;
                        const int c = it2->second;
                        const int pc = (int) chemin[c].size();
                        const long long n_pc = compte[c];
                        pile.push_back({c, SA + n_pc, SA2 + (long long)(2*pc - 1) * n_pc});
                    }
                }
                cands_scored += ncand;
#ifdef OBRAIN_DIFF_DUMP
                if (OBRAIN_DIFF_HIT) {
                    fprintf(stderr, "OBRAIN_DIFF_END_LI trace=%d ordinal=%d li=%d L=%d candidate_count=%d "
                                    "best_propose=%d best_P=%d best_N=%d best_SA=%lld best_SA2=%lld early_return=%d\n",
                            g_diff_trace, g_diff_ordinal, li, L, ncand, best.propose ? 1 : 0, best.P, best.N,
                            best.sum_A, best.sum_A2, best.propose ? 1 : 0);
                }
#endif
                if (best.propose) { best.candidate_count = ncand; break; }   // plus long suffixe d'abord, comme l'oracle
            }

        if (!best.propose) abstain_count++;
        return best;
    }

    // ---- SITE 2 : Update(State_t, accepted[0:C]). SEULS les jetons COMMITTED. ----
    void update(const std::vector<int32_t> & accepted) {
        for (int32_t tok : accepted) {
            ctx.push_back(tok);
            committed_updates++;
            alimenter();
        }
    }

    int64_t state_token_count() const { return (int64_t) ctx.size(); }

    // [R3-A4] tout increment d'output_tokens PASSE PAR ICI : impossible d'en oublier un
    // slot_appelant = identifiant du SLOT SERVEUR qui emet l'evenement (pas notre etat interne).
    // ASSERTION ANTI-POLLUTION : si notre etat est initialise, l'evenement doit venir de NOTRE contexte.
    void obrain_count_output(int tok, const char * type, int is_stop, int is_eog,
                             int64_t slot_appelant, int64_t gen_appelante) {
        if (initialized && (slot_appelant != slot_id || gen_appelante != generation)) {
            fprintf(stderr, "OBRAIN_INVARIANT_KO [anti-pollution] evenement du contexte (%lld,%lld) "
                            "consomme par le contexte (%lld,%lld) -- FATAL\n",
                    (long long) slot_appelant, (long long) gen_appelante,
                    (long long) slot_id, (long long) generation);
            abort();
        }
        if (!initialized) {
            // contexte pas encore initialise : l'evenement appartient au PRE-INIT de CE slot.
            g_pre_init_by_ctx[g_pre_init_ctx_key(slot_appelant, gen_appelante)].push_back((int32_t) tok);
            obrain_ledger_add((int) slot_appelant, type, tok, 0, committed_updates, 0, is_stop, is_eog);
            return;                       // NE TOUCHE PAS aux compteurs de ce contexte
        }
        output_tokens_counted++;
        obrain_ledger_add((int) slot_id, type, tok, output_tokens_counted, committed_updates,
                          state_token_count(), is_stop, is_eog);
    }
    int64_t output_tokens_post_init() const { return output_tokens_counted - pre_init_output_events; }

    uint64_t state_digest() const {   // canonique et LOGIQUE : aucun accident physique
        uint64_t x = 1469598103934665603ull;
        auto mix = [&x](uint64_t v) { x ^= v; x *= 1099511628211ull; };
        mix((uint64_t) ctx.size());
        for (int c : insere_par_L) mix((uint64_t) c);
        for (size_t li = 0; li < buckets.size(); ++li) {
            std::vector<uint64_t> cles;
            cles.reserve(buckets[li].size());
            for (auto & kv : buckets[li]) cles.push_back(kv.first);
            std::sort(cles.begin(), cles.end());
            mix((uint64_t) li); mix((uint64_t) cles.size());
            for (uint64_t c : cles) { mix(c); mix((uint64_t) buckets[li].at(c).size()); }
        }
        return x;
    }

    // ---- invariants FATALS ----
    bool check_invariants(const char * ou) const {
        // [R3-A5] le temoin est le RESUMÉ DU LEDGER : output_tokens_counted = |O|.
        // AVANT Init, la frontiere n'est pas encore posee : tout OUTPUT est de classe "prompt".
        const int64_t pre = pre_init_output_events;
        if (committed_updates != output_tokens_counted) {
            fprintf(stderr, "OBRAIN_INVARIANT_KO [%s] committed_updates=%lld != |O_post_init|=%lld "
                            "(pre_init=%lld, initialized=%d)\n",
                    ou, (long long) committed_updates, (long long) (output_tokens_counted - pre),
                    (long long) pre, (int) initialized);
            return false;
        }
        const int64_t attendu = prompt_tokens_inserted + committed_updates;
        if (state_token_count() != attendu) {
            fprintf(stderr, "OBRAIN_INVARIANT_KO [%s] state_token_count=%lld != prompt=%lld + committed=%lld\n",
                    ou, (long long) state_token_count(), (long long) prompt_tokens_inserted,
                    (long long) committed_updates);
            return false;
        }
        if (committed_updates != normal_commit_tokens + spec_commit_tokens) {
            fprintf(stderr, "OBRAIN_INVARIANT_KO [%s] committed_updates=%lld != normal=%lld + spec=%lld"
                            " (DOUBLE INSERTION ?)\n",
                    ou, (long long) committed_updates, (long long) normal_commit_tokens,
                    (long long) spec_commit_tokens);
            return false;
        }
        if (rejected_draft_updates != 0) {
            fprintf(stderr, "OBRAIN_INVARIANT_KO [%s] rejected_draft_updates=%lld != 0\n",
                    ou, (long long) rejected_draft_updates);
            return false;
        }
        return true;
    }
};
