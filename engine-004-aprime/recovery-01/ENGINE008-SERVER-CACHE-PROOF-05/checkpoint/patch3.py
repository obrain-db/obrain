import re
p="/home/triviere/projects/llama-obrain-cuda-f1a/tools/server/server-context.cpp"
s=open(p,encoding="utf-8",errors="surrogateescape").read()
old='            task.jspace_extract_only = body.value("extract_only", false);           // [obrain K1] extraire → ref'
assert old in s, "A1 route"
s=s.replace(old, old+"""
            // [obrain K1-IDS] IDs NATIFS du prefixe (mode de transfert exact).
            // Borne explicite : 1 a 4096 IDs. Refuse une entree textuelle
            // concurrente, pour qu'aucune tokenisation implicite n'ait lieu.
            if (body.contains("fact_token_ids")) {
                if (!body["fact_token_ids"].is_array()) {
                    res->error(format_error_response("'fact_token_ids' must be an array of ints", ERROR_TYPE_INVALID_REQUEST));
                    return res;
                }
                if (body.contains("fact_text")) {
                    res->error(format_error_response("'fact_token_ids' and 'fact_text' are mutually exclusive", ERROR_TYPE_INVALID_REQUEST));
                    return res;
                }
                const size_t n = body["fact_token_ids"].size();
                if (n < 1 || n > 4096) {
                    res->error(format_error_response("'fact_token_ids' length must be 1..4096, got " + std::to_string(n), ERROR_TYPE_INVALID_REQUEST));
                    return res;
                }
                task.jspace_fact_token_ids.reserve(n);
                const int32_t n_vocab_ids = llama_vocab_n_tokens(ctx_server.vocab);
                for (const auto & v : body["fact_token_ids"]) {
                    const int id = v.get<int>();
                    if (id < 0 || id >= n_vocab_ids) {
                        res->error(format_error_response("fact_token_ids contains an out-of-range id " + std::to_string(id), ERROR_TYPE_INVALID_REQUEST));
                        return res;
                    }
                    task.jspace_fact_token_ids.push_back((llama_token) id);
                }
            }""",1)
open(p,"w",encoding="utf-8",errors="surrogateescape").write(s)
print("route : fact_token_ids parse (borne 1..4096, exclusif de fact_text, ids verifies)")
