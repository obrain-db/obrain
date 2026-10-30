#!/usr/bin/env python3
# ENGINE-005C : inscrit l'indice de couche dans le NOM du tenseur, au moment de la construction
# du graphe. C'est le point que l'instrumentation de timing ne pouvait pas deviner : le callback
# cb() recoit `il` mais ne renomme pas le tenseur.
#
# Usage : python3 apply-layer-name.py <src/llama-graph.cpp>
import sys, os

ANCHOR = """void llm_graph_context::cb(ggml_tensor * cur, const char * name, int il) const {
    if (cb_func) {
        cb_func(ubatch, cur, name, il);
    }
}"""

REPL = """void llm_graph_context::cb(ggml_tensor * cur, const char * name, int il) const {
    // [ENGINE-005C] inscription de la couche dans le nom : le callback recoit `il`, mais le nom
    // du tenseur ne le portait pas. Ajout PUR d'un suffixe (aucun autre effet) : la mesure par
    // couche peut alors lire l'information sans reconstruction.
    if (cur != nullptr && il >= 0) {
        char obs_buf[GGML_MAX_NAME];
        snprintf(obs_buf, sizeof(obs_buf), "%s_l%d", cur->name, il);
        ggml_set_name(cur, obs_buf);
    }
    if (cb_func) {
        cb_func(ubatch, cur, name, il);
    }
}"""

def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: apply-layer-name.py <src/llama-graph.cpp>")
    path = sys.argv[1]
    text = open(path, encoding="utf-8").read()
    if "[ENGINE-005C]" in text:
        raise SystemExit("DEJA PATCHE")
    n = text.count(ANCHOR)
    if n != 1:
        raise SystemExit("ANCRE INTROUVABLE OU AMBIGUE : %d" % n)
    text = text.replace(ANCHOR, REPL, 1)
    tmp = path + ".e5c-tmp"
    open(tmp, "w", encoding="utf-8").write(text)
    os.replace(tmp, path)
    print("ECRIT %s (%d octets)" % (path, len(text)))

if __name__ == "__main__":
    main()
