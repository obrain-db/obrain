import re
F="/home/triviere/projects/llama-obrain-cuda-f1a"
# ---------- 1. TACHE : IDs natifs du prefixe ----------
p=F+"/tools/server/server-task.h"
s=open(p,encoding="utf-8",errors="surrogateescape").read()
old='    std::string jspace_state_ref;             // ref hex d\'un état extrait (inject par ref)'
assert old in s, "A1 task"
s=s.replace(old, old+"""
    // [obrain K1-IDS] IDs NATIFS du prefixe a transferer, fournis par l'appelant.
    // Sert a imposer EXACTEMENT les 59 IDs autoritaires, sans repasser par le
    // template du serveur (qui produit 7 tokens pour le meme texte). Non vide
    // => cette liste est utilisee TELLE QUELLE et `fact_text` est IGNORE.
    std::vector<llama_token> jspace_fact_token_ids;""",1)
open(p,"w",encoding="utf-8",errors="surrogateescape").write(s)
print("1. task.h : jspace_fact_token_ids")

# ---------- 2. RESULTAT : compteurs de la requete courante ----------
p2=F+"/tools/server/server-task.h"
s2=open(p2,encoding="utf-8",errors="surrogateescape").read()
m=re.search(r'(struct server_task_result_jspace_inject : server_task_result \{\n)', s2)
assert m, "A2 result"
s2=s2[:m.end()]+"""    // [obrain K1-IDS] COMPTEURS DE LA REQUETE COURANTE. Distinguent ce qui a
    // REELLEMENT ete decode pendant CETTE requete de ce qui vient d'un etat
    // importe. Sur un import par `state_ref`, les deux valent 0 : le prefixe
    // n'est pas rejoue. Sur `extract_only` / chemin texte, ils comptent le fait.
    int32_t     prefix_decode_calls = 0;   // appels de decode portant le prefixe
    int32_t     prefix_token_count  = 0;   // tokens de prefixe decodes ici
    bool        state_imported      = false; // true si l'etat vient d'un import
"""+s2[m.end():]
open(p2,"w",encoding="utf-8",errors="surrogateescape").write(s2)
print("2. task.h : compteurs de requete")

# ---------- 3. HANDLER : IDs natifs + compteurs ----------
p3=F+"/tools/server/server-context.cpp"
s3=open(p3,encoding="utf-8",errors="surrogateescape").read()
old3='                    std::vector<llama_token> fact_tokens  = common_tokenize(ctx, task.jspace_fact_text, true, true);'
assert old3 in s3, "A3 handler tokenize"
new3="""                    // [obrain K1-IDS] IDs NATIFS fournis par l'appelant : utilises
                    // TELS QUELS, sans seconde tokenisation. `fact_text` est alors
                    // ignore -- une entree textuelle concurrente est REFUSEE.
                    std::vector<llama_token> fact_tokens;
                    if (!task.jspace_fact_token_ids.empty()) {
                        if (!task.jspace_fact_text.empty()) {
                            auto res = std::make_unique<server_task_result_error>();
                            res->id = task.id;
                            res->err_type = ERROR_TYPE_INVALID_REQUEST;
                            res->err_msg = "fact_token_ids and fact_text are mutually exclusive";
                            queue_results.send(std::move(res));
                            break;
                        }
                        fact_tokens = task.jspace_fact_token_ids;
                    } else {
                        fact_tokens = common_tokenize(ctx, task.jspace_fact_text, true, true);
                    }"""
s3=s3.replace(old3,new3,1)

# compteurs : initialises, puis renseignes aux deux endroits de decode du prefixe
m=re.search(r'( +)int32_t n_fact = \(int32_t\) fact_tokens\.size\(\);', s3)
assert m, "A4 n_fact"
I=m.group(1)
s3=s3[:m.end()]+f"""
{I}// [obrain K1-IDS] compteurs de la requete courante
{I}int32_t cur_prefix_decode_calls = 0;
{I}int32_t cur_prefix_token_count  = 0;"""+s3[m.end():]

old5='                        if (decode_batch(fact_tokens, 0, false) != 0) {'
assert old5 in s3, "A5 decode"
s3=s3.replace(old5,"""                        cur_prefix_decode_calls = 1;
                        cur_prefix_token_count  = (int32_t) fact_tokens.size();
                        if (decode_batch(fact_tokens, 0, false) != 0) {""",1)

# sur le chemin by-ref : marquer l'import
old6='                        n_fact = (int32_t) blob.n_tokens;'
assert old6 in s3, "A6 n_fact byref"
s3=s3.replace(old6, old6+"""
                        cur_state_imported = true;""",1)
# declarer cur_state_imported a cote des compteurs
s3=s3.replace("int32_t cur_prefix_token_count  = 0;",
              "int32_t cur_prefix_token_count  = 0;\n"+I+"bool    cur_state_imported      = false;",1)

# publier dans le resultat : aux deux points d'envoi
s3=s3.replace("                    res->fact_tokens = n_fact;",
"""                    res->fact_tokens = n_fact;
                    res->prefix_decode_calls = cur_prefix_decode_calls;
                    res->prefix_token_count  = cur_prefix_token_count;
                    res->state_imported      = cur_state_imported;""")
open(p3,"w",encoding="utf-8",errors="surrogateescape").write(s3)
print("3. handler : IDs natifs + compteurs publies")
