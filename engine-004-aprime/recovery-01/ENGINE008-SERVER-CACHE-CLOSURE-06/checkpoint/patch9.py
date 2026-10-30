p="/home/triviere/projects/llama-obrain-cuda-f1a/tools/server/server-context.cpp"
s=open(p,encoding="utf-8",errors="surrogateescape").read()
old='    m.model_path  = std::string(llama_model_desc(model));'
assert old in s, "A1"
new='''    { char dbuf[512] = {0};
      const int dn = llama_model_desc(model, dbuf, sizeof dbuf);
      m.model_path = (dn > 0) ? std::string(dbuf) : std::string("<indisponible>"); }'''
s=s.replace(old,new,1)
open(p,"w",encoding="utf-8",errors="surrogateescape").write(s)
print("llama_model_desc avec buffer")
