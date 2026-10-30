p="/home/triviere/projects/llama-obrain-cuda-f1a/tools/server/server-context.cpp"
s=open(p,encoding="utf-8",errors="surrogateescape").read()
old='''        if (!body.contains("fact_text") || !body["fact_text"].is_string()) {
            if (!kv_ref_route) {
                res->error(format_error_response("'fact_text' must be a string (or pass 'state_ref')", ERROR_TYPE_INVALID_REQUEST));
                return res;
            }
        }'''
assert old in s, "A1 validation"
new='''        // [obrain K1-IDS] `fact_token_ids` est une source de prefixe VALIDE, au
        // meme titre que `fact_text`. Sans cette ligne, une requete n'envoyant
        // que des IDs natifs etait refusee avant meme d'atteindre le parsing.
        const bool kv_ids_route = body.contains("fact_token_ids") && body["fact_token_ids"].is_array()
                                  && !body["fact_token_ids"].empty();
        if ((!body.contains("fact_text") || !body["fact_text"].is_string()) && !kv_ids_route) {
            if (!kv_ref_route) {
                res->error(format_error_response("'fact_text' must be a string, or pass 'state_ref', or pass 'fact_token_ids'", ERROR_TYPE_INVALID_REQUEST));
                return res;
            }
        }'''
s=s.replace(old,new,1)
open(p,"w",encoding="utf-8",errors="surrogateescape").write(s)
print("validation : fact_token_ids accepte")
