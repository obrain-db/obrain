#pragma once

#include "common.h"
#include "llama.h"

#include <string>
#include <unordered_set>
#include <list>
#include <map>
#include <memory>

// TODO: prevent including the whole server-common.h as we only use server_tokens
#include "server-common.h"

// [obrain SPLICE-CHAT] obrain_kv::Blob (etat KV + manifeste de compatibilite).
//
// DECLARATION AVANCEE VOLONTAIRE, PAS UN INCLUDE : obrain-kv-cache.h tire
// obrain-sha256.h, dont le corps de `hex_of`/`hex_of_file` depend de
// OBRAIN_USE_OPENSSL_SHA256 — define PRIVE de la cible server-context. Inclure
// cet en-tete ici le ferait compiler SANS le define par des cibles qui incluent
// server-task.h sans OpenSSL (llama-server-impl, main.cpp), soit DEUX
// definitions inline differentes du meme symbole : violation ODR, et risque de
// selection silencieuse de l'implementation lente. `std::shared_ptr<T>` reste
// valide avec un T INCOMPLET (contrairement a unique_ptr) : la tache transporte
// le blob sans avoir besoin de le connaitre ici.
namespace obrain_kv { struct Blob; }

using json = nlohmann::ordered_json;

// [obrain SPLICE-CHAT/K1b] Repertoire de persistance des etats KV (fichiers
// <ref>.bin, format OBKV). UNE SEULE definition de la valeur par defaut : la
// route jspace et le flux de chat lisent donc le MEME repertoire par
// construction (le defaut est relatif au CWD du serveur).
static constexpr const char * DEFAULT_KV_STATES_DIR = "kv-states";

enum server_task_type {
    SERVER_TASK_TYPE_COMPLETION,
    SERVER_TASK_TYPE_EMBEDDING,
    SERVER_TASK_TYPE_RERANK,
    SERVER_TASK_TYPE_INFILL,
    SERVER_TASK_TYPE_CANCEL,
    SERVER_TASK_TYPE_CONTROL,
    SERVER_TASK_TYPE_NEXT_RESPONSE,
    SERVER_TASK_TYPE_METRICS,
    SERVER_TASK_TYPE_SLOT_SAVE,
    SERVER_TASK_TYPE_SLOT_RESTORE,
    SERVER_TASK_TYPE_SLOT_ERASE,
    SERVER_TASK_TYPE_SLOT_COMPACT, // [obrain LONG-RESUME] phantom-prefix compaction
    SERVER_TASK_TYPE_GET_LORA,
    SERVER_TASK_TYPE_SET_LORA,
    SERVER_TASK_TYPE_SET_ATTN_MASK,
    SERVER_TASK_TYPE_JSPACE_COMPARE_GENERATION,
    SERVER_TASK_TYPE_JSPACE_INJECT_KNOWLEDGE,
};

// TODO: change this to more generic "response_format" to replace the "format_response_*" in server-common
enum task_response_type {
    TASK_RESPONSE_TYPE_NONE, // llama.cpp native format
    TASK_RESPONSE_TYPE_OAI_CHAT,
    TASK_RESPONSE_TYPE_OAI_CMPL,
    TASK_RESPONSE_TYPE_OAI_RESP,
    TASK_RESPONSE_TYPE_OAI_ASR, // transcriptions API
    TASK_RESPONSE_TYPE_OAI_EMBD,
    TASK_RESPONSE_TYPE_ANTHROPIC,
};

enum stop_type {
    STOP_TYPE_NONE,
    STOP_TYPE_EOS,
    STOP_TYPE_WORD,
    STOP_TYPE_LIMIT,
};

struct task_params {
    bool stream          = false;
    bool include_usage   = false;
    bool cache_prompt    = true; // remember the prompt to avoid reprocessing all prompt
    bool return_tokens   = false;
    bool return_progress = false;

    int32_t sse_ping_interval = 30; // seconds between SSE comment pings while the stream stays silent, -1 disables

    int32_t n_keep    =  0; // number of tokens to keep from initial prompt
    int32_t n_discard =  0; // number of tokens after n_keep that may be discarded when shifting context, 0 defaults to half
    int32_t n_predict = -1; // new tokens to predict
    int32_t n_indent  =  0; // minimum line indentation for the generated text in number of whitespace characters
    int32_t n_cmpl    =  1; // number of completions to generate from this prompt

    int32_t n_cache_reuse = 0; // min chunk size to attempt reusing from the cache via KV shifting (0 = disabled)

    int64_t t_max_prompt_ms  = -1; // TODO: implement
    int64_t t_max_predict_ms = -1; // if positive, limit the generation phase to this time limit

    std::map<int, float> lora; // mapping adapter ID -> scale

    std::vector<std::string> antiprompt;
    std::vector<std::string> response_fields;

    bool timings_per_token   = false;
    bool post_sampling_probs = false;

    // [obrain J2] read-only confidence probe: per-token top-k entropy of the sampler
    // candidates, summarized in the final response. never alters the generation.
    bool confidence_probe = false;

    struct common_params_sampling sampling;
    struct common_params_speculative speculative;

    // [obrain J3] raw texts to seed the speculative n-gram structures with before
    // generation (schema-agnostic: opaque strings, no assumption about their origin)
    std::vector<std::string> spec_seed_texts;

    // response formatting
    bool               verbose  = false;
    task_response_type res_type = TASK_RESPONSE_TYPE_NONE;
    std::string        oaicompat_model;
    std::string        oaicompat_cmpl_id;

    // realtime control (SERVER_TASK_TYPE_CONTROL)
    std::string        control_action;
    std::string        control_cmpl_id;

    // per-request parameters for chat parsing
    common_chat_parser_params chat_parser_params;

    // message spans for checkpointing
    common_chat_msg_spans message_spans;

    // Embeddings
    int32_t embd_normalize = 2; // (-1=none, 0=max absolute int16, 1=taxicab, 2=Euclidean/L2, >2=p-norm)

    json format_logit_bias(const std::vector<llama_logit_bias> & logit_bias) const;
    json to_json(bool only_metrics = false) const;
};

// struct for tracking the state of a task (e.g., for streaming)
struct task_result_state {
    // tracking diffs for partial tool calls
    std::vector<common_chat_msg_diff> diffs;
    common_chat_parser_params chat_parser_params;
    common_chat_msg chat_msg;
    std::string generated_text; // append new chunks of generated text here
    std::vector<std::string> generated_tool_call_ids;
    std::unordered_set<size_t> sent_tool_call_names;

    // for OpenAI Responses and Anthropic streaming API:
    // track output item / content block state across chunks
    bool thinking_block_started = false;
    bool text_block_started = false;

    // for OpenAI Responses streaming API
    bool oai_resp_created = false;
    const std::string oai_resp_id;
    const std::string oai_resp_reasoning_id;
    const std::string oai_resp_message_id;
    std::string oai_resp_fc_id; // function call ID for current args delta

    task_result_state(const common_chat_parser_params & chat_parser_params);

    // parse partial tool calls and update the internal state
    common_chat_msg update_chat_msg(
        const std::string & text_added,
        bool is_partial,
        std::vector<common_chat_msg_diff> & diffs,
        bool filter_tool_calls = false);
};

struct server_task {
    int id = -1; // to be filled by server_queue

    // TODO @ngxson : remove this field and implement a mapping task_id -> idx in the response_reader
    size_t index = 0; // used when there are multiple prompts (batch request)

    // used by SERVER_TASK_TYPE_CANCEL
    int id_target = -1;
    int id_slot   = -1;

    // [obrain P1] IDENTITE FACULTATIVE DU CLIENT (champ `client_id` du corps de
    // requete). Elle sert de cle au registre client -> sequence : deux
    // conversations qui fournissent deux cid distincts obtiennent deux
    // sequences distinctes, meme si leurs `id_slot` haches sont identiques.
    // Vide = identite derivee du `id_slot` (comportement inchange).
    std::string client_id;

    // used by parallel sampling (multiple completions from same prompt)
    int id_parent  = -1;
    // temporary store of child tasks for scheduling
    // note: accessing to elements is invalid after the task is moved to server_slot
    std::vector<server_task> child_tasks;

    // used by SERVER_TASK_TYPE_INFERENCE
    task_params   params;
    server_tokens tokens;

    // only used by CLI, this allow tokenizing CLI inputs on server side
    // we need this because mtmd_context and vocab are not accessible outside of server_context
    bool                    cli = false;
    std::string             cli_prompt;
    std::vector<raw_buffer> cli_files;

    server_task_type type;

    // used by SERVER_TASK_TYPE_SLOT_SAVE, SERVER_TASK_TYPE_SLOT_RESTORE, SERVER_TASK_TYPE_SLOT_ERASE
    struct slot_action {
        int id_slot;
        std::string filename;
        std::string filepath;
        // [obrain LONG-RESUME] used by SERVER_TASK_TYPE_SLOT_COMPACT:
        // keep only the last `tail_m` attention cells (-1 = info only, no removal)
        int32_t tail_m = -1;
    };
    slot_action slot_action;

    // used by SERVER_TASK_TYPE_METRICS
    bool metrics_reset_bucket = false;

    // used by SERVER_TASK_TYPE_SET_LORA
    std::map<int, float> set_lora; // mapping adapter ID -> scale

    // used by SERVER_TASK_TYPE_SET_ATTN_MASK
    std::vector<float>     attn_mask;     // [n_pos * n_pos], 0.0f=visible, -inf=masked
    std::vector<llama_pos> attn_mask_pos; // [n_pos]
    int32_t attn_mask_n_head_groups = 0;
    int32_t attn_mask_slot_id = -1;

    // used by SERVER_TASK_TYPE_JSPACE_COMPARE_GENERATION [obrain]
    std::string jspace_prompt;
    float       jspace_scale      = 0.15f; // perturbation magnitude, fraction of ||h||_rms
    int32_t     jspace_n_generate = 24;    // tokens to generate per branch

    // used by SERVER_TASK_TYPE_JSPACE_INJECT_KNOWLEDGE [obrain] — PO plan
    // 4749fb72, tâche P5-T5. jspace_prompt/jspace_n_generate above are
    // REUSED here as the query prompt / generation length (kept separate
    // fields only for the fact text, which has no equivalent in
    // compare_generation).
    std::string jspace_fact_text;
    // [obrain K1/K2] état K/V pré-calculé — BYPASS injection texte.
    std::string jspace_state_ref;             // ref hex d'un état extrait (inject par ref)
    // [obrain K1-IDS] IDs NATIFS du prefixe a transferer, fournis par l'appelant.
    // Sert a imposer EXACTEMENT les 59 IDs autoritaires, sans repasser par le
    // template du serveur (qui produit 7 tokens pour le meme texte). Non vide
    // => cette liste est utilisee TELLE QUELLE et `fact_text` est IGNORE.
    std::vector<llama_token> jspace_fact_token_ids;
    bool        jspace_extract_only = false;  // extraire l'état du texte → ref (pas de génération)

    // ═══════════════════════════════════════════════════════════════════════════
    // [obrain APPEND-K3] INJECTION INCREMENTALE PAR PAGE.
    //
    // POURQUOI CES CHAMPS EXISTENT. Un contexte de 1M jetons coute 65 556
    // octets/jeton de KV (f16) plus 156 894 360 octets d'etat recurrent, soit
    // ~65,7 Go par client. Le seul moyen de le recharger sans payer ce cout a
    // chaque tour est de l'assembler PAR PAGES : chaque page est un etat KV
    // borne a une plage de positions [pos_min, pos_max], et l'assemblage AJOUTE
    // les pages au lieu de remplacer la sequence.
    //
    // `jspace_state_append` : la requete AJOUTE une page (drapeau
    // LLAMA_STATE_SEQ_FLAGS_APPEND) au lieu de REMPLACER la sequence. La route
    // ne purge PAS la sequence reservee, ne decode AUCUNE question, ne genere
    // RIEN : elle rend l'etat de la sequence apres l'ajout.
    //
    // `jspace_pos_min` / `jspace_pos_max` : la plage de positions ANNONCEE par
    // l'appelant pour la page. Elle est verifiee contre le manifeste du blob ET
    // contre la sequence cible AVANT tout appel au moteur : une plage fausse
    // rend un HTTP 400 lisible au lieu d'un refus profond.
    // ═══════════════════════════════════════════════════════════════════════════
    bool        jspace_state_append  = false;
    int64_t     jspace_pos_min       = -1;   // -1 = non annoncee
    int64_t     jspace_pos_max       = -1;

    // [obrain APPEND-K3] REMISE A ZERO EXPLICITE de la sequence avant l'ajout.
    //
    // POURQUOI CE CHAMP EST NECESSAIRE, ET PAS UN CONFORT : la sequence reservee
    // est PARTAGEE par toutes les requetes J-space (extraction comme ajout). Une
    // extraction y laisse le prefixe qu'elle vient de decoder ; l'ajout suivant
    // est alors refuse, a juste titre, comme non contigu (« la page commence a 0
    // alors que la sequence attend 2456 »). Mesure le 2026-10-05 : c'est
    // exactement ce qui arrive si l'on enchaine une extraction et un ajout sans
    // remise a zero. Plutot que de purger en silence (ce qui casserait
    // l'accumulation, qui est tout l'interet de l'ajout), la remise a zero est
    // DEMANDEE explicitement par l'appelant.
    bool        jspace_state_reset   = false;

    // [obrain APPEND-K3] EXTRACTION D'UNE PAGE, PAS D'UN PREFIXE COMPLET.
    //
    // `jspace_pos_start` : position du PREMIER jeton du prefixe decode (defaut 0
    // = comportement historique). Avec pos_start = N, le fait est decode en
    // [N, N + n_fact) et l'etat extrait est la PAGE [N, N + n_fact).
    //
    // `jspace_prerequis_refs` : pages a AJOUTER a la sequence reservee AVANT de
    // decoder la nouvelle page. POURQUOI C'EST NECESSAIRE SUR UN MODELE HYBRIDE :
    // l'etat recurrent est un PLI de tout ce que la sequence a vu. Une page 1
    // extraite d'une session qui n'a jamais vu la page 0 porterait un pli qui
    // l'ignore -- l'ajouter ensuite produirait un etat recurrent FAUX, donc des
    // reponses fausses SANS AUCUN MESSAGE. En rejouant les pages precedentes
    // avant le decodage, le pli couvre bien [0, pos_max] ; l'extraction annonce
    // alors `recr_from = 0`, et la route K3 acceptera la page.
    int64_t     jspace_pos_start = 0;
    std::vector<std::string> jspace_prerequis_refs;
    // [obrain O1] mode de verification EXPLICITEMENT active : conserve les IDs
    // reellement selectionnes et les premiers logits natifs.
    bool        jspace_verify = false;
    std::string jspace_kv_dir = DEFAULT_KV_STATES_DIR;  // répertoire de persistance des états (K1b)
    // 2026-07-13 fix (operator: "on doit identifier et traiter la
    // deformation") — the original implementation ALWAYS used the
    // stochastic sampler (temp=0.7, fixed seed=42) inherited verbatim
    // from JSPACE_COMPARE_GENERATION, which is a research/comparison tool
    // where sampling noise is intentional (it reveals more than argmax
    // flips). The production inject_knowledge endpoint silently ignored
    // any caller-provided temperature (no such field even existed) and
    // reused that same noisy sampler for FACTUAL RECALL, where determinism
    // is what's actually needed. Reproduced live : injecting "ROUGE-7"
    // then asking "what is the code?" deterministically returned the
    // WRONG "ROUGE-8" three times in a row at the (ignored) temperature=0
    // caller request -- because the real, hardcoded sampler was never
    // greedy to begin with. Default true = greedy/deterministic, the
    // right default for recall ; false preserves the original stochastic
    // behavior for comparison-style experimentation.
    bool jspace_greedy = true;

    // [obrain SPLICE-CHAT] MARCHE 2 — SPLICE INVISIBLE DEPUIS LE FLUX DE CHAT.
    //
    // POURQUOI ICI ET PAS DANS jspace_state_ref : la route jspace GENERE sa
    // propre boucle de decodage sur la sequence reservee (elle ne streame pas).
    // Le flux de chat, lui, doit streamer exactement comme aujourd'hui : on ne
    // remplace donc PAS la boucle de generation, on rend seulement le slot
    // capable de demarrer son prefill APRES un etat KV deja present.
    //
    // `state_blob` est resolu par la COUCHE HTTP (cache memoire puis fichier
    // <dir>/<ref>.bin, meme source que jspace) : l'etat n'est lu qu'UNE fois,
    // et une requete `n_cmpl > 1` partage le meme blob entre ses taches filles
    // au lieu de relire/recopier l'etat pour chacune.
    //
    // INVARIANT (tenu par handle_completions_impl) : les IDs du prefixe du
    // manifeste sont PREPENDES a `tokens`. Autrement dit, une tache spliCee se
    // lit comme une tache dont le prompt COMMENCE par le fait — ce qui est
    // exactement la semantique J-space (le fait occupe les positions
    // [0, n_state_prefix)) et ce qui laisse INTACTE la regle d'indexation du
    // serveur : slot.prompt.tokens[i] <-> task.tokens[i] <-> position i.
    std::shared_ptr<const obrain_kv::Blob> state_blob;

    // Ref d'origine (journalisation et messages d'erreur : la ref est la CLE du
    // cache et le nom du fichier persistant, elle doit apparaitre telle quelle).
    std::string state_ref;

    // Nombre de tokens du prefixe importe (0 = AUCUN splice : chemin
    // historique, aucun cout, aucun branchement nouveau).
    int32_t n_state_prefix = 0;

    // Vrai si CETTE tache demarre apres un etat KV importe.
    bool spliced() const {
        return n_state_prefix > 0;
    }

    server_task() = default;

    server_task(server_task_type type) : type(type) {}

    int32_t n_tokens() const {
        return tokens.size();
    }

    bool need_embd() const {
        switch (type) {
            case SERVER_TASK_TYPE_EMBEDDING:
            case SERVER_TASK_TYPE_RERANK:
                return true;
            default:
                return false;
        }
    }

    bool need_logits() const {
        switch (type) {
            case SERVER_TASK_TYPE_COMPLETION:
            case SERVER_TASK_TYPE_INFILL:
                return true;
            default:
                return false;
        }
    }

    bool need_sampling() const {
        switch (type) {
            case SERVER_TASK_TYPE_COMPLETION:
            case SERVER_TASK_TYPE_INFILL:
                return true;
            default:
                return false;
        }
    }

    // utility function
    static std::unordered_set<int> get_list_id(const std::vector<server_task> & tasks) {
        std::unordered_set<int> ids(tasks.size());
        for (size_t i = 0; i < tasks.size(); i++) {
            ids.insert(tasks[i].id);
            for (auto & child : tasks[i].child_tasks) {
                ids.insert(child.id);
            }
        }
        return ids;
    }

    void add_child(int id_parent, int id_child) {
        server_task copy;

        copy.id        = id_child;
        copy.id_parent = id_parent;
        copy.params    = params;
        copy.type      = type;
        copy.tokens    = tokens.clone();
        copy.id_slot   = -1; // child tasks cannot specify slot
        // [obrain P1] une tache FILLE n'a pas d'identite client : elle partage
        // le KV de son parent (son prefixe vient de lui), elle ne peut donc pas
        // reclamer une sequence propre sans dupliquer ce contexte.
        copy.client_id.clear();

        // [obrain SPLICE-CHAT] les taches filles heritent du prefixe : `tokens`
        // clone ci-dessus contient deja les IDs du fait, et le blob resolu est
        // PARTAGE (shared_ptr) plutot que relu pour chacune. Une tache fille
        // dont le blob serait absent (autre chemin) n'est PAS cassee pour
        // autant : sans blob, elle est prefillee normalement, prefixe compris,
        // par la suite de la boucle de prefill — degrade, jamais incorrect.
        copy.state_blob      = state_blob;
        copy.state_ref       = state_ref;
        copy.n_state_prefix  = n_state_prefix;

        // use different sampling seed for each child
        // note: https://github.com/ggml-org/llama.cpp/pull/18700#discussion_r2675115723
        if (copy.params.sampling.seed != LLAMA_DEFAULT_SEED) {
            copy.params.sampling.seed += (uint32_t)child_tasks.size() + 1;
        }

        child_tasks.push_back(std::move(copy));
    }

    // the task will be moved into queue, then onto slots
    // however, the state must be kept by caller (e.g., HTTP thread)
    task_result_state create_state() const {
        return task_result_state(params.chat_parser_params);
    }

    bool is_parent() const {
        return child_tasks.size() > 0;
    }

    bool is_child() const {
        return id_parent != -1;
    }
};

struct result_timings {
    int32_t cache_n = -1;

    int32_t prompt_n = -1;
    double prompt_ms = 0.0;
    double prompt_per_token_ms = 0.0;
    double prompt_per_second = 0.0;

    int32_t predicted_n = -1;
    double predicted_ms = 0.0;
    double predicted_per_token_ms = 0.0;
    double predicted_per_second = 0.0;

    // Optional speculative metrics - only included when > 0
    int32_t draft_n = 0;
    int32_t draft_n_accepted = 0;

    json to_json() const;
};

struct result_prompt_progress {
    int32_t total = 0;
    int32_t cache = 0;
    int32_t processed = 0;
    int64_t time_ms = 0;

    json to_json() const;
};

struct server_task_result {
    int id           = -1;
    int id_slot      = -1;

    // TODO @ngxson : remove this field and implement a mapping task_id -> idx in the response_reader
    size_t index = 0; // to be used for batched tasks

    virtual bool is_error() {
        // only used by server_task_result_error
        return false;
    }
    virtual bool is_stop() {
        // only used by server_task_result_cmpl_*
        return true;
    }
    virtual void update(task_result_state &) {
        // only used by server_task_result_cmpl_*
    }
    virtual json to_json() = 0;
    virtual ~server_task_result() = default;
    virtual server_task_result * clone() const {
        GGML_ABORT("not implemented for this task type");
    }
};

// using shared_ptr for polymorphism of server_task_result
using server_task_result_ptr = std::unique_ptr<server_task_result>;

struct completion_token_output {
    llama_token tok;
    float prob;
    std::string text_to_send;
    struct prob_info {
        llama_token tok;
        std::string txt;
        float prob;
    };
    std::vector<prob_info> probs;

    json to_json(bool post_sampling_probs) const;

    static json probs_vector_to_json(const std::vector<completion_token_output> & probs, bool post_sampling_probs);

    static float logarithm(float x);

    static std::vector<unsigned char> str_to_bytes(const std::string & str);

};

struct server_task_result_cmpl_final : server_task_result {
    std::string content;
    llama_tokens tokens;

    bool stream;
    bool include_usage;
    result_timings timings;
    std::string prompt;

    bool truncated;
    int32_t n_decoded;
    int32_t n_prompt_tokens;
    int32_t n_prompt_tokens_cache;
    int32_t n_tokens_cached;
    bool has_new_line;
    std::string stopping_word;
    stop_type stop = STOP_TYPE_NONE;

    bool post_sampling_probs;
    std::vector<completion_token_output> probs_output;
    std::vector<std::string>  response_fields;

    // [obrain J2] confidence probe summary (null when the probe is inactive)
    json confidence;

    task_params generation_params;

    // response formatting
    bool               verbose  = false;
    task_response_type res_type = TASK_RESPONSE_TYPE_NONE;
    std::string        oaicompat_model;
    std::string        oaicompat_cmpl_id;
    common_chat_msg    oaicompat_msg; // to be populated by update()

    std::vector<common_chat_msg_diff> oaicompat_msg_diffs; // to be populated by update()
    bool is_updated = false;

    // for OpenAI Responses API
    std::string oai_resp_id;
    std::string oai_resp_reasoning_id;
    std::string oai_resp_message_id;

    virtual bool is_stop() override {
        return true; // in stream mode, final responses are considered stop
    }

    virtual json to_json() override;

    virtual void update(task_result_state & state) override {
        is_updated = true;
        oaicompat_msg = state.update_chat_msg(content, false, oaicompat_msg_diffs);

        oai_resp_id = state.oai_resp_id;
        oai_resp_reasoning_id = state.oai_resp_reasoning_id;
        oai_resp_message_id = state.oai_resp_message_id;
    }

    json to_json_non_oaicompat();

    json usage_json_oaicompat();

    json to_json_oaicompat();

    json to_json_oaicompat_chat();

    json to_json_oaicompat_chat_stream();

    json to_json_oaicompat_resp();

    json to_json_oaicompat_resp_stream();

    json to_json_oaicompat_asr();

    json to_json_anthropic();

    json to_json_anthropic_stream();
};

struct server_task_result_cmpl_partial : server_task_result {
    std::string  content;
    llama_tokens tokens;

    int32_t n_decoded;
    int32_t n_prompt_tokens;
    int32_t n_prompt_tokens_cache;

    bool post_sampling_probs;
    bool is_progress = false;
    bool is_begin = false; // whether to send 200 status to HTTP client (begin of SSE stream)
                           // ref: https://github.com/ggml-org/llama.cpp/pull/23884
    completion_token_output prob_output;
    result_timings timings;
    result_prompt_progress progress;

    // response formatting
    bool               verbose  = false;
    task_response_type res_type = TASK_RESPONSE_TYPE_NONE;
    std::string        oaicompat_model;
    std::string        oaicompat_cmpl_id;
    std::vector<common_chat_msg_diff> oaicompat_msg_diffs; // to be populated by update()
    bool is_updated = false;

    // Streaming state copied from task_result_state for this chunk
    bool thinking_block_started = false;
    bool text_block_started     = false;

    // for OpenAI Responses API
    bool oai_resp_created = false;
    std::string oai_resp_id;
    std::string oai_resp_reasoning_id;
    std::string oai_resp_message_id;
    std::string oai_resp_fc_id;

    // for Anthropic API: track if any reasoning content has been generated
    bool anthropic_has_reasoning = false;

    virtual bool is_stop() override {
        return false; // in stream mode, partial responses are not considered stop
    }

    virtual void update(task_result_state & state) override;

    virtual json to_json() override;

    json to_json_non_oaicompat();

    json to_json_oaicompat();

    json to_json_oaicompat_chat();

    json to_json_oaicompat_resp();

    json to_json_oaicompat_asr();

    json to_json_anthropic();
};

struct server_task_result_embd : server_task_result {
    std::vector<std::vector<float>> embedding;

    int32_t n_tokens;

    // response formatting
    task_response_type res_type = TASK_RESPONSE_TYPE_NONE;

    virtual json to_json() override;

    json to_json_non_oaicompat();

    json to_json_oaicompat();
};

struct server_task_result_rerank : server_task_result {
    float score = -1e6;

    int32_t n_tokens;

    virtual json to_json() override;
};

struct server_task_result_error : server_task_result {
    error_type err_type = ERROR_TYPE_SERVER;
    std::string err_msg;

    // for ERROR_TYPE_EXCEED_CONTEXT_SIZE
    int32_t n_prompt_tokens = 0;
    int32_t n_ctx           = 0;

    virtual bool is_error() override {
        return true;
    }

    virtual json to_json() override;
};

struct server_task_result_metrics : server_task_result {
    int n_idle_slots;
    int n_processing_slots;
    int n_tasks_deferred;
    int64_t t_start;

    // TODO: somehow reuse server_metrics in the future, instead of duplicating the fields
    uint64_t n_prompt_tokens_processed_total = 0;
    uint64_t t_prompt_processing_total       = 0;
    uint64_t n_tokens_predicted_total        = 0;
    uint64_t t_tokens_generation_total       = 0;

    uint64_t n_tokens_max = 0;

    uint64_t n_prompt_tokens_processed = 0;
    uint64_t t_prompt_processing       = 0;

    uint64_t n_tokens_predicted  = 0;
    uint64_t t_tokens_generation = 0;

    uint64_t n_decode_total     = 0;
    uint64_t n_busy_slots_total = 0;

    // while we can also use std::vector<server_slot> this requires copying the slot object which can be quite messy
    // therefore, we use json to temporarily store the slot.to_json() result
    json slots_data = json::array();

    virtual json to_json() override;
};

struct server_task_result_slot_save_load : server_task_result {
    std::string filename;
    bool is_save; // true = save, false = load

    size_t n_tokens;
    size_t n_bytes;
    double t_ms;

    virtual json to_json() override;
};

// [obrain LONG-RESUME] result of SERVER_TASK_TYPE_SLOT_COMPACT.
// Reports the attention-cell window before/after the phantom-prefix compaction.
struct server_task_result_slot_compact : server_task_result {
    bool    compacted = false;
    int32_t tail_m = -1;
    int32_t n_ctx = 0;   // capacité du slot (cellules) — pour les seuils RELATIFS côté client
    int64_t cells_before = 0;
    int64_t cells_after = 0;
    int64_t pos_min = -1;
    int64_t pos_max = -1;
    size_t  n_prompt_tokens = 0;

    virtual json to_json() override {
        return json {
            { "compacted",       compacted },
            { "tail_m",          tail_m },
            { "n_ctx",           n_ctx },
            { "cells_before",    cells_before },
            { "cells_after",     cells_after },
            { "pos_min",         pos_min },
            { "pos_max",         pos_max },
            { "n_prompt_tokens", n_prompt_tokens },
        };
    }
};

struct server_task_result_slot_erase : server_task_result {
    size_t n_erased;

    virtual json to_json() override;
};

struct server_task_result_control : server_task_result {
    bool        success = false;
    std::string message; // optional detail when success is false

    virtual json to_json() override {
        json out = json { { "success", success } };
        if (!message.empty()) {
            out["message"] = message;
        }
        return out;
    }
};

struct server_task_result_get_lora : server_task_result {
    struct lora {
        common_adapter_lora_info info;
        std::string  alora_invocation_string;
        llama_tokens alora_invocation_tokens;
    };
    std::vector<lora> loras;

    virtual json to_json() override;
};

struct server_task_result_apply_lora : server_task_result {
    virtual json to_json() override;
};

struct server_task_result_set_attn_mask : server_task_result {
    virtual json to_json() override;
};

// [obrain] result of SERVER_TASK_TYPE_JSPACE_COMPARE_GENERATION
struct server_task_result_jspace_compare : server_task_result {
    std::string text_baseline;
    std::string text_eigen;
    std::string text_random;
    float  h_rms         = 0.0f;
    float  top_eigenvalue = 0.0f;
    double gram_seconds   = 0.0;
    virtual json to_json() override;
};

// [obrain] result of SERVER_TASK_TYPE_JSPACE_INJECT_KNOWLEDGE — PO plan
// 4749fb72, tâche P5-T5. Production entry point for the KV-splice
// mechanism validated in llm-engine (notes 4392e92f et al.): the fact's
// tokens are processed once, its full KV/recurrent state is extracted and
// round-tripped (extract+reinject) into the same reserved sequence, then
// the query prompt is decoded and generation proceeds — the fact's TEXT
// is never part of the query's visible context, only its resulting state.
struct server_task_result_jspace_inject : server_task_result {
    // [obrain K1-IDS] COMPTEURS DE LA REQUETE COURANTE. Distinguent ce qui a
    // REELLEMENT ete decode pendant CETTE requete de ce qui vient d'un etat
    // importe. Sur un import par `state_ref`, les deux valent 0 : le prefixe
    // n'est pas rejoue. Sur `extract_only` / chemin texte, ils comptent le fait.
    int32_t     prefix_decode_calls = 0;   // appels de decode portant le prefixe
    int32_t     prefix_token_count  = 0;   // tokens de prefixe decodes ici
    bool        state_imported      = false; // true si l'etat vient d'un import
    // [obrain O1] MODE DE VERIFICATION. Renseigne UNIQUEMENT si task.jspace_verify.
    // Les IDs viennent de la generation de PRODUCTION (llama_sampler_sample),
    // jamais d'une retokenisation du texte.
    bool        verify_enabled        = false;
    std::string first_logits_base64;   // premiers logits NATIFS, copies avant invalidation
    int32_t     first_logits_n        = 0;
    std::string decoded_ids_json;      // IDs reellement decodes, dans l'ordre
    std::string decoded_pos_json;      // positions correspondantes
    int32_t     continuation_decode_calls = 0;
    int32_t     continuation_token_count  = 0;
    int32_t     selected_ids_count   = 0;   // IDs ENREGISTRES (un par selection)
    int32_t     emitted_ids_count    = 0;   // IDs EMIS dans le texte
    int32_t     n_generate_requested = 0;   // PLAFOND demande, PAS le nombre produit
    std::string selection_method;      // methode de selection effective
    std::string stop_reason;           // regle d'arret effective
    // [obrain K1-METRICS] mesures SEPAREES. Convention : >= 0 = mesure reelle ;
    //                        -1 = NON APPLICABLE ; -2 = NON MESURE.
    float ms_prefix_compute   = -2.0f;
    float ms_state_lookup     = -2.0f;
    float ms_model_sha256     = -2.0f;
    float ms_blob_sha256      = -2.0f;
    float ms_state_import     = -2.0f;
    float ms_compatibility    = -2.0f;
    float ms_preparation_total= -2.0f;
    std::string text;
    int32_t     fact_tokens = 0;
    size_t      blob_bytes  = 0;
    // [obrain SPLICE-BOUND] LES DEUX PARTS DU BLOB, mesurees sur les octets
    // reellement extraits. `blob_bytes` seul ne dit pas si un blob de 158 Mo
    // est « trop de jetons » (defaut de bornage) ou « etat recurrent d'un
    // modele hybride » (taille intrinsique) : deux causes, deux correctifs
    // opposes. Ces trois champs rendent la distinction observable de
    // l'exterieur, sans avoir a relire le blob.
    size_t      kv_bytes        = 0;   // part KV (proportionnelle aux jetons)
    size_t      recurrent_bytes = 0;   // part recurrente (constante)
    uint32_t    n_attn_cells    = 0;   // cellules KV de la sequence extraite
    // 2026-07-13 (operator: "fais ce qu'il faut", re: CUDA graph reuse
    // investigation) — per-generated-token decode latency, in ms. jspace's
    // reserved-slot path emits none of the normal slot print_timing /
    // "graphs reused" logging (verified live it never fires for this
    // path), so there was no clean, in-process way to tell whether CUDA
    // graph capture/replay is actually kicking in for the generation loop
    // (batch shape is stable since commit 4426450cd's fix, which is a
    // precondition for graph reuse, but stable shape alone doesn't PROVE
    // reuse is happening). If graph capture+replay is active, expect a
    // clear pattern: first entry (or first few) notably slower (capture),
    // then a flat, much lower plateau (replay). A flat, undifferentiated
    // series across all tokens would mean no reuse benefit is showing up
    // here — informative either way, and directly observable per call
    // instead of inferred from noisy wall-clock HTTP timing across
    // separate requests.
    std::vector<float> per_token_ms;
    float fact_decode_ms  = 0.0f;
    float query_decode_ms = 0.0f;
    // [obrain SPEC-inject] consommateur speculatif de la boucle inject
    // (--spec-inject-consume) : tokens draftes / acceptes pendant la
    // generation de ce call. 0/0 quand le flag ou le speculatif est absent —
    // c est la mesure A/B du chantier (acceptance > 0 attendue en citation
    // d un fait seede, ~0 sans le flag).
    int32_t spec_draft_n        = 0;
    int32_t spec_draft_accepted = 0;

    // [obrain APPEND-K3] RESULTAT D'UN AJOUT DE PAGE. POURQUOI CES CHAMPS :
    // sans eux, un ajout reussi et un ajout qui aurait silencieusement remplace
    // la sequence seraient indistinguables de l'exterieur. `import_bytes` est la
    // MESURE du gain -- il ne compte QUE la page, jamais le prefixe deja en
    // place -- et `seq_pos_*` prouve l'etat de la sequence apres l'ajout.
    bool           state_append     = false;
    int64_t        page_pos_min     = -1;
    int64_t        page_pos_max     = -1;
    int64_t        seq_pos_min      = -1;   // positions de la sequence APRES l'ajout
    int64_t        seq_pos_max      = -1;
    int64_t        attn_pos_min     = -1;   // idem, vue ATTENTION seule (modele hybride)
    int64_t        attn_pos_max     = -1;
    uint64_t       seq_cells        = 0;    // cellules presentes apres l'ajout
    size_t         import_bytes     = 0;    // octets REELLEMENT importes (la page)
    int32_t        recr_from        = -1;   // 1re position couverte par l'etat recurrent de la page
    virtual json to_json() override;
};

struct server_prompt_data {
    std::vector<uint8_t> main;
    std::vector<uint8_t> drft;

    size_t size() const {
        return main.size() + drft.size();
    }
};

struct server_prompt {
    server_tokens tokens;

    server_prompt_data data;

    std::list<common_prompt_checkpoint> checkpoints;

    size_t size() const {
        size_t res = 0;

        res += data.size();

        for (const auto & ckpt : checkpoints) {
            res += ckpt.size();
        }

        return res;
    }

    int n_tokens() const {
        return tokens.size();
    }

    server_prompt clone() const {
        return server_prompt {
            tokens.clone(),
            data,
            checkpoints,
        };
    }
};

struct server_prompt_cache {
    server_prompt_cache(int32_t limit_size_mib, size_t limit_tokens) {
        this->limit_size   = 1024ull*1024ull*(limit_size_mib < 0 ? 0 : limit_size_mib);
        this->limit_tokens = limit_tokens;
    }

    std::list<server_prompt> states;

    // in bytes, 0 = no limit
    size_t limit_size = 0;

    // in tokens, 0 = no limit
    size_t limit_tokens = 0;

    size_t size() const;

    size_t n_tokens() const;

    server_prompt * alloc(const server_prompt & prompt, size_t state_size_main, size_t state_size_drft);

    bool load(server_prompt & prompt, const server_tokens & tokens_new, llama_context * ctx_main, llama_context * ctx_drft, int32_t id_slot);

    void update();
};

// used exclusively by router mode
struct server_task_result_router : server_task_result {
    json data;
    virtual json to_json() override { return data; }
    virtual server_task_result * clone() const override {
        return new server_task_result_router(*this);
    }
};
