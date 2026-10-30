p="/home/triviere/projects/llama-obrain-cuda-f1a/tools/server/server-task.cpp"
s=open(p,encoding="utf-8",errors="surrogateescape").read()
old='''        { "spec_draft_n",        spec_draft_n },
        { "spec_draft_accepted", spec_draft_accepted },
    };
}'''
assert old in s, "A1 to_json"
new='''        { "spec_draft_n",        spec_draft_n },
        { "spec_draft_accepted", spec_draft_accepted },
        // [obrain K1-IDS] compteurs de la REQUETE COURANTE : distinguent ce qui a
        // ete decode ici de ce qui vient d'un etat importe.
        { "prefix_decode_calls", prefix_decode_calls },
        { "prefix_token_count",  prefix_token_count },
        { "state_imported",      state_imported },
    };
}'''
s=s.replace(old,new,1)
open(p,"w",encoding="utf-8",errors="surrogateescape").write(s)
print("to_json : compteurs publies")
