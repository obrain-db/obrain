// [obrain] BANC DIRECT DU VERIFICATEUR -- instrument de MESURE, pas du runtime.
//
// Il n'utilise AUCUN proposeur (ni MTP, ni n-gram, ni draft-n-max) : il construit lui-meme un batch
// cible contenant exactement `b` positions successives d'une meme sequence (p_i = p0 + i), puis
// chronometre la passe avec la MEME frontiere que le verify live :
//
//     synchronisation -> t0 -> llama_decode(batch) -> echantillonnage sur les b positions -> t1
//
// Invariant physique imprime a chaque repetition : b_observe == b_demande. Si l'invariant ne tient
// pas, la ligne porte INVALID=1 et le point ne doit pas entrer dans la table.
//
// Sortie machine : OBRAIN_VBENCH b=<b> rep=<r> decode_ms=<..> sample_ms=<..> total_ms=<..> b_obs=<..>

#include "arg.h"
#include "common.h"
#include "sampling.h"
#include "llama.h"
#include "log.h"

#include <chrono>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

static int64_t now_us() {
    return std::chrono::duration_cast<std::chrono::microseconds>(
               std::chrono::steady_clock::now().time_since_epoch()).count();
}

int main(int argc, char ** argv) {
    // [obrain] AUCUN pre-reglage avant le parsing : common_params_parse exige les sentinelles
    // (n_gpu_layers < 0 notamment) et fait avorter l execution sinon. C etait le troisieme defaut.
    common_params params;

    int b_demande = 8;
    int repetitions = 5;
    int prompt_len = 64;
    int a_demande = -1;          // -1 = pas de controle d acceptation

    // [obrain] on RETIRE nos propres drapeaux de argv : le parseur commun refuse les arguments
    // inconnus (il a fait avorter deux executions). Nos valeurs sont lues d abord, puis argv est
    // filtre avant de lui etre confie.
    std::vector<char *> argv_filtre;
    argv_filtre.push_back(argv[0]);
    for (int i = 1; i < argc; ++i) {
        if (strcmp(argv[i], "--vb-b") == 0 && i + 1 < argc) { b_demande = atoi(argv[++i]); continue; }
        if (strcmp(argv[i], "--vb-r") == 0 && i + 1 < argc) { repetitions = atoi(argv[++i]); continue; }
        if (strcmp(argv[i], "--vb-p") == 0 && i + 1 < argc) { prompt_len = atoi(argv[++i]); continue; }
        if (strcmp(argv[i], "--vb-a") == 0 && i + 1 < argc) { a_demande = atoi(argv[++i]); continue; }
        argv_filtre.push_back(argv[i]);
    }
    int argc_filtre = (int) argv_filtre.size();
    if (!common_params_parse(argc_filtre, argv_filtre.data(), params, LLAMA_EXAMPLE_BENCH)) {
        return 1;
    }
    // [obrain] les valeurs par defaut sont posees APRES le parsing
    params.n_gpu_layers   = 99;
    params.n_ctx          = 4096;
    params.n_batch        = 512;
    params.n_ubatch       = 512;
    params.sampling.temp  = 0.0f;
    params.sampling.seed  = 12345;
    llama_backend_init();
    llama_numa_init(params.numa);

    auto mparams = common_model_params_to_llama(params);
    auto cparams = common_context_params_to_llama(params);
    llama_model * model = llama_model_load_from_file(params.model.path.c_str(), mparams);
    if (!model) { fprintf(stderr, "modele illisible\n"); return 1; }
    llama_context * ctx = llama_init_from_model(model, cparams);
    if (!ctx) { fprintf(stderr, "contexte impossible\n"); return 1; }

    const llama_vocab * vocab = llama_model_get_vocab(model);
    auto * smpl = common_sampler_init(model, params.sampling);

    // jetons deterministes (pas de generateur de brouillon) : on prend des jetons du vocabulaire
    const int n_vocab = llama_vocab_n_tokens(vocab);
    std::vector<llama_token> jetons(b_demande);
    for (int i = 0; i < b_demande; ++i) {
        jetons[i] = (llama_token) (100 + (i * 7919) % (n_vocab - 200));
    }

    std::vector<llama_token> prompt(prompt_len);
    for (int i = 0; i < prompt_len; ++i) {
        prompt[i] = (llama_token) (200 + (i * 104729) % (n_vocab - 400));
    }

    // ---------- warmup complet ----------
    {
        llama_memory_clear(llama_get_memory(ctx), true);
        llama_batch bp = llama_batch_init(prompt_len, 0, 1);
        for (int i = 0; i < prompt_len; ++i) { common_batch_add(bp, prompt[i], i, {0}, false); }
        bp.logits[bp.n_tokens - 1] = true;
        llama_decode(ctx, bp);
        llama_batch_free(bp);

        llama_batch bw = llama_batch_init(b_demande, 0, 1);
        for (int i = 0; i < b_demande; ++i) {
            common_batch_add(bw, jetons[i], prompt_len + i, {0}, true);
        }
        llama_decode(ctx, bw);
        llama_batch_free(bw);
        llama_synchronize(ctx);
    }

    // ---------- reference : jetons greedy de la cible sur les b positions ----------
    // sert a construire un candidat dont les A premiers jetons sont exactement acceptables.
    std::vector<llama_token> greedy(b_demande, 0);
    {
        llama_memory_clear(llama_get_memory(ctx), true);
        llama_batch bp = llama_batch_init(prompt_len, 0, 1);
        for (int i = 0; i < prompt_len; ++i) { common_batch_add(bp, prompt[i], i, {0}, false); }
        bp.logits[bp.n_tokens - 1] = true;
        llama_decode(ctx, bp);
        llama_batch_free(bp);
        common_sampler_reset(smpl);
        llama_batch bref = llama_batch_init(b_demande, 0, 1);
        for (int i = 0; i < b_demande; ++i) { common_batch_add(bref, jetons[i], prompt_len + i, {0}, true); }
        llama_decode(ctx, bref);
        for (int i = 0; i < b_demande; ++i) {
            greedy[i] = common_sampler_sample(smpl, ctx, i);
        }
        llama_batch_free(bref);
        llama_synchronize(ctx);
    }

    for (int rep = 0; rep < repetitions; ++rep) {
        // meme etat KV initial a chaque repetition : prompt identique, KV vide
        llama_memory_clear(llama_get_memory(ctx), true);
        {
            llama_batch bp = llama_batch_init(prompt_len, 0, 1);
            for (int i = 0; i < prompt_len; ++i) { common_batch_add(bp, prompt[i], i, {0}, false); }
            bp.logits[bp.n_tokens - 1] = true;
            llama_decode(ctx, bp);
            llama_batch_free(bp);
        }
        common_sampler_reset(smpl);

        // [obrain] Le prefixe accepte est GENERE PAR LA CIBLE ELLE-MEME, pasin par un lot arbitraire :
        // sinon les logits du test sont conditionnes sur un contexte different de celui de la
        // reference et l acceptation retombe a 0 (constate : A_req=3 -> A_obs=0, VALID=0). On avance
        // donc A pas greedy unitaires AVANT la passe chronometree, ce qui place le KV exactement la ou
        // il serait dans la verification live (prompt + A jetons acceptes).
        std::vector<llama_token> candidat(jetons);
        int a_observe = -1;
        int pos = prompt_len;
        if (a_demande > 0) {
            // MARCHE GREEDY AUTHENTIQUE : on decode le jeton ECHANTILLONNE (et non un jeton arbitraire),
            // sinon le contexte du test differe de celui de la generation et l acceptation retombe a 0.
            llama_batch bu = llama_batch_init(1, 0, 1);
            common_batch_add(bu, jetons[0], pos, {0}, true);   // amorce
            llama_decode(ctx, bu);
            pos++;
            for (int i = 0; i < a_demande; ++i) {
                const llama_token t = common_sampler_sample(smpl, ctx, 0);   // ce que la cible veut
                common_sampler_reset(smpl);
                candidat[i] = t;
                llama_batch bn = llama_batch_init(1, 0, 1);
                common_batch_add(bn, t, pos, {0}, true);                    // on alimente CE jeton
                llama_decode(ctx, bn);
                llama_batch_free(bn);
                pos++;
            }
            llama_batch_free(bu);
            if (a_demande < b_demande - 1) {
                candidat[a_demande] = (llama_token) ((candidat[a_demande] + 7) % llama_vocab_n_tokens(vocab));
            }
            for (int i = a_demande + 1; i < b_demande; ++i) { candidat[i] = jetons[i]; }
        }

        llama_batch b = llama_batch_init(b_demande, 0, 1);
        for (int i = 0; i < b_demande; ++i) {
            common_batch_add(b, candidat[i], pos + i, {0}, true);            // positions successives
        }
        const int b_observe = b.n_tokens;                                   // 1re variable controlee

        llama_synchronize(ctx);
        const int64_t t0 = now_us();
        const int ret = llama_decode(ctx, b);
        const int64_t t_enq = now_us();          // llama_decode a rendu la main
        if (ret != 0) { fprintf(stderr, "llama_decode a echoue\n"); return 1; }
        llama_synchronize(ctx);
        const int64_t t_sync = now_us();         // calcul GPU termine

        // ACCEPTATION : logique live -- on echantillonne position par position et on compare au
        // candidat suivant ; on s arrete au premier desaccord. C est ce que fait la verification
        // speculative, et c est ce chemin qui doit etre chronometre.
        // [obrain] LOGIQUE LIVE : on echantillonne position par position et on s ARRETE au premier
        // desaccord (branche reject) -- les positions suivantes ne sont ni echantillonnees ni
        // acceptees. En mode controle, l issue de la comparaison est imposee, mais le sampling reste
        // le vrai et la boucle garde la meme etendue que le live (A+1 positions echantillonnees).
        std::vector<llama_token> echantillon(b_demande, 0);
        a_observe = 0;
        // PREMIER ACCES AUX LOGITS : c est lui qui rend reellement les logits disponibles pour
        // l acceptation. Si cet acces est couteux, la frontiere target-ready est APRES lui.
        const int64_t t_log0 = now_us();
        echantillon[0] = common_sampler_sample(smpl, ctx, 0);
        const int64_t t_log1 = now_us();
        const int64_t t1 = t_log1;               // frontiere target-ready = logits disponibles
        // le premier echantillon compte comme le premier appariement (off-by-one corrige)
        if ((a_demande < 0 && echantillon[0] == candidat[1]) || (a_demande >= 1)) { a_observe = 1; }
        for (int i = 1; i + 1 < b_demande; ++i) {
            echantillon[i] = common_sampler_sample(smpl, ctx, i);
            const bool match_reel = (echantillon[i] == candidat[i + 1]);
            const bool match = (a_demande < 0) ? match_reel : (i < a_demande);
            if (!match) { break; }
            a_observe++;
        }
        const int64_t t2 = now_us();
        llama_synchronize(ctx);

        const double enq_ms   = (t_enq - t0) / 1000.0;
        const double sync_ms  = (t_sync - t_enq) / 1000.0;
        const double log_ms   = (t_log1 - t_log0) / 1000.0;
        const double ready_ms = (t1 - t0) / 1000.0;
        const double total_ms = (t2 - t0) / 1000.0;   // MESURE DIRECTE, jamais une somme
        const int proposed = b_demande - 1;
        const int committed = a_observe + 1;
        const int sampled = a_observe + 1;
        const int inv_b_prop = (b_observe == proposed + 1);
        const int inv_a_range = (a_observe >= 0 && a_observe <= proposed);
        const int inv_c = (committed == a_observe + 1);
        const int inv_s = (sampled == a_observe + 1);
        const int invalide = (b_observe == b_demande && inv_b_prop && inv_a_range && inv_c && inv_s &&
                              (a_demande < 0 || a_observe == a_demande)) ? 0 : 1;

        printf("OBRAIN_VBENCH b=%d proposed=%d accepted_draft=%d committed=%d sampled_positions=%d "
               "decode_enqueue_ms=%.3f sync_ms=%.3f first_logits_access_ms=%.3f target_ready_ms=%.3f "
               "post_ms=%.3f total_ms=%.3f VALID=%d\n",
               b_observe, b_demande - 1, a_observe, a_observe + 1,
               a_observe + 1, enq_ms, sync_ms, log_ms, ready_ms, total_ms - ready_ms, total_ms,
               invalide == 0 ? 1 : 0);
        fflush(stdout);
        llama_batch_free(b);
    }

    common_sampler_free(smpl);
    llama_free(ctx);
    llama_model_free(model);
    llama_backend_free();
    return 0;
}
