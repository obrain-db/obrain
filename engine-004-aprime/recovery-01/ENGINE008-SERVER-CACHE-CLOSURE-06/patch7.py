import re
p="/home/triviere/projects/llama-obrain-cuda-f1a/tools/server/server-context.cpp"
s=open(p,encoding="utf-8",errors="surrogateescape").read()

# ---------- EXTRACTION : construire et stocker le manifeste ----------
old='''                            const std::string ref = kv_ref_of(data);
                            {
                                std::lock_guard<std::mutex> lk(g_kv_states_mtx);
                                // Aucune entree de cache si l insertion est REFUSEE
                                // (blob vide, ou plus gros que la borne memoire).
                                if (!kv_states_insert(ref, kv_state_blob{(uint32_t) n_fact, data})) {'''
assert old in s, "A1 extract"
new='''                            const std::string ref = kv_ref_of(data);
                            // [obrain K1-MANIFEST] manifeste construit ICI, la ou les
                            // identites et la configuration sont effectives.
                            std::shared_ptr<kv_state_manifest> man = std::make_shared<kv_state_manifest>(
                                kv_manifest_of(ctx, ctx_server.model_tgt, ctx_server.params_base.model,
                                               (uint32_t) llama_n_ctx(ctx), (uint32_t) llama_n_batch(ctx),
                                               (uint32_t) llama_n_ubatch(ctx), seq_id));
                            man->format_magic = 0xaf143cd9u;   // io_magic du format de blob
                            man->blob_bytes   = data.size();
                            man->blob_sha256  = kv_sha256_hex(data.data(), data.size());
                            man->n_tokens     = (int32_t) n_fact;
                            man->prefix_ids   = std::vector<llama_token>(fact_tokens.begin(), fact_tokens.begin() +
                                                         std::min<size_t>(fact_tokens.size(), 4096));
                            { llama_memory_t mm = llama_get_memory(ctx);
                              man->pos_min = llama_memory_seq_pos_min(mm, seq_id);
                              man->pos_max = llama_memory_seq_pos_max(mm, seq_id); }
                            {
                                std::lock_guard<std::mutex> lk(g_kv_states_mtx);
                                // Aucune entree de cache si l insertion est REFUSEE
                                // (blob vide, ou plus gros que la borne memoire).
                                kv_state_blob kb{(uint32_t) n_fact, data, man};
                                if (!kv_states_insert(ref, std::move(kb))) {'''
s=s.replace(old,new,1)

# ---------- METRIQUES SEPAREES ----------
old2='''                    const size_t blob_size = llama_state_seq_get_size(ctx, seq_id);
                    const float fact_decode_ms = (float) (ggml_time_us() - t_fact_decode_start) / 1000.0f;'''
assert old2 in s, "A2 metrics"
new2='''                    const size_t blob_size = llama_state_seq_get_size(ctx, seq_id);
                    // [obrain K1-METRICS] METRIQUES SEPAREES. `fact_decode_ms` est
                    // conserve pour compatibilite, mais il ne dit PAS ce qu'il
                    // mesure : sur le chemin `state_ref` il couvre l'import, sur le
                    // chemin texte il couvre le decodage du fait. Les champs
                    // ci-dessous separent les deux, et `-1` signifie NON MESURE --
                    // jamais un zero implicite.
                    const float preparation_total_ms = (float) (ggml_time_us() - t_fact_decode_start) / 1000.0f;
                    const float fact_decode_ms = preparation_total_ms;
                    cur_ms_prefix_compute = (cur_prefix_decode_calls > 0) ? fact_decode_ms : -1.0f;
                    cur_ms_state_lookup   = cur_state_imported ? -1.0f : -1.0f;  // renseigne plus bas si import
                    cur_ms_integrity      = -1.0f;
                    cur_ms_state_import   = -1.0f;
                    cur_ms_preparation    = preparation_total_ms;'''
s=s.replace(old2,new2,1)

# declarations des metriques
old3='bool    cur_state_imported      = false;'
assert old3 in s, "A3 counters"
s=s.replace(old3, old3+'''
                    float   cur_ms_prefix_compute = -1.0f;   // -1 = NON MESURE
                    float   cur_ms_state_lookup   = -1.0f;
                    float   cur_ms_integrity      = -1.0f;
                    float   cur_ms_state_import   = -1.0f;
                    float   cur_ms_preparation    = -1.0f;''',1)
open(p,"w",encoding="utf-8",errors="surrogateescape").write(s)
print("PATCH 7 : manifeste a l'extraction + metriques separees")
