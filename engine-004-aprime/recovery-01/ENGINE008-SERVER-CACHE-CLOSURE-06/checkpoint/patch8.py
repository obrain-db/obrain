import re
p="/home/triviere/projects/llama-obrain-cuda-f1a/tools/server/server-context.cpp"
s=open(p,encoding="utf-8",errors="surrogateescape").read()

# 1. decouper le bloc manifeste et le deplacer AVANT le struct blob
m=re.search(r'// \[obrain K1-MANIFEST\] MANIFESTE DE COMPATIBILITE.*?\n(?=static std::mutex g_kv_states_mtx;)', s, re.S)
assert m, "bloc manifeste introuvable"
bloc=m.group(0)
s=s[:m.start()]+s[m.end():]

# 2. signature : plus de ctx_server ni de chemin ; tout vient de ctx
bloc=bloc.replace('''static kv_state_manifest kv_manifest_of(llama_context * ctx, llama_model * model,
                                        const std::string & model_path,
                                        uint32_t n_ctx, uint32_t n_batch, uint32_t n_ubatch,
                                        int32_t seq_id) {
    kv_state_manifest m;
    m.model_path  = model_path;
    m.model_bytes = 0;
    { std::ifstream f(model_path, std::ios::binary | std::ios::ate);
      if (f) m.model_bytes = (size_t) f.tellg(); }''',
'''static kv_state_manifest kv_manifest_of(llama_context * ctx, int32_t seq_id) {
    const llama_model * model = llama_get_model(ctx);
    kv_state_manifest m;
    // Identite du modele SANS acces fichier : description + taille + topologie.
    m.model_path  = std::string(llama_model_desc(model));
    m.model_bytes = (size_t) llama_model_size(model);''')
bloc=bloc.replace('''    m.n_ctx = n_ctx; m.n_batch = n_batch; m.n_ubatch = n_ubatch; m.n_seq_max = 1;''',
'''    m.n_ctx = (uint32_t) llama_n_ctx(ctx);
    m.n_batch = (uint32_t) llama_n_batch(ctx);
    m.n_ubatch = (uint32_t) llama_n_ubatch(ctx);
    m.n_seq_max = 1;''')
assert 'kv_manifest_of(llama_context * ctx, int32_t seq_id)' in bloc

# 3. inserer AVANT le struct blob
anchor='// [obrain K1/K2] états K/V extraits'
assert anchor in s, "ancre blob"
s=s.replace(anchor, bloc+anchor, 1)

# 4. corriger les appels
s=s.replace('''                            std::shared_ptr<kv_state_manifest> man = std::make_shared<kv_state_manifest>(
                                kv_manifest_of(ctx, ctx_server.model_tgt, ctx_server.params_base.model,
                                               (uint32_t) llama_n_ctx(ctx), (uint32_t) llama_n_batch(ctx),
                                               (uint32_t) llama_n_ubatch(ctx), seq_id));''',
'''                            std::shared_ptr<kv_state_manifest> man = std::make_shared<kv_state_manifest>(
                                kv_manifest_of(ctx, seq_id));''')
s=s.replace('''                            const kv_state_manifest cur = kv_manifest_of(
                                ctx, ctx_server.model_tgt, ctx_server.params_base.model,
                                (uint32_t) llama_n_ctx(ctx), (uint32_t) llama_n_batch(ctx),
                                (uint32_t) llama_n_ubatch(ctx), seq_id);''',
'''                            const kv_state_manifest cur = kv_manifest_of(ctx, seq_id);''')
open(p,"w",encoding="utf-8",errors="surrogateescape").write(s)
print("PATCH 8 : manifeste deplace avant le blob, derive de ctx seulement")
