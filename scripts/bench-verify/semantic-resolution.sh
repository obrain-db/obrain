#!/usr/bin/env bash
# Bench "résolution sémantique qui apprend" : précision/rappel des arêtes CALLS
# obrain-indexer (M6, substrate store) vs indexeur C legacy (codebase-memory-mcp,
# base SQLite), vérité terrain relative.
#
# Conçu pour être lancé PAR scripts/bench-verify/guard.sh (étape « résolution
# sémantique ») : exit≠0 quand un seuil n'est pas atteint ou quand le bench ne
# peut pas s'exécuter (pas de faux vert). Codes de sortie :
#   0 = métriques calculées et ≥ seuils ; 1 = métriques < seuils ;
#   2 = infra manquante (store/db/binaire) ou erreur interne.
# Seuils (fractions) : SEMANTIC_MIN_PRECISION et SEMANTIC_MIN_RECALL, défaut 0.5.
# Mettre les deux à 0 pour une simple mesure (pas de verdict).
#
# Usage :
#   scripts/bench-verify/semantic-resolution.sh
# Env configurables :
#   OBRAIN_BIN  binaire obrain (défaut target/release/obrain)
#   SEMANTIC_STORE  store substrat M6 (défaut /tmp/engine.obrain). Variable
#                   DÉDIÉE : l'OBRAIN_STORE ambiant (ex. harness runtime) est
#                   volontairement ignoré pour éviter un faux vert/rouge.
#   C_DB        base SQLite C (défaut ~/.cache/codebase-memory-mcp/obrain-engine.db)
#   C_PROJECT   projet SQLite (défaut obrain-engine)
#   REPO_ROOT   racine du repo pour relativiser les file_path (défaut $(pwd))
#   DOMAIN      préfixe relatif des fichiers comparés (défaut crates/obrain-engine/)
#   SEMANTIC_MIN_PRECISION / SEMANTIC_MIN_RECALL  seuils de garde (défaut 0.5)
set -u
cd "$(dirname "$0")/../.."
ROOT=$(pwd)
OBRAIN_BIN=${OBRAIN_BIN:-$ROOT/target/release/obrain}
# Knob dédié : ne JAMAIS hériter d'un OBRAIN_STORE ambiant (harness) — sinon le
# bench mesurerait un autre store sans s'en apercevoir (faux vert / faux rouge).
SEMANTIC_STORE=${SEMANTIC_STORE:-/tmp/engine.obrain}
C_DB=${C_DB:-$HOME/.cache/codebase-memory-mcp/obrain-engine.db}
C_PROJECT=${C_PROJECT:-obrain-engine}
REPO_ROOT=${REPO_ROOT:-$ROOT}
DOMAIN=${DOMAIN:-crates/obrain-engine/}

[ -x "$OBRAIN_BIN" ] || { echo "obrain bin absent: $OBRAIN_BIN (cargo build --release ?)" >&2; exit 2; }
[ -d "$SEMANTIC_STORE" ] || { echo "store M6 absent: $SEMANTIC_STORE" >&2; exit 2; }
[ -f "$C_DB" ] || { echo "db C absente: $C_DB" >&2; exit 2; }

export SEMANTIC_STORE
export OBRAIN_BIN C_DB C_PROJECT REPO_ROOT DOMAIN

python3 - <<'PYEOF'
import json, os, sqlite3, subprocess, sys, collections

BIN   = os.environ['OBRAIN_BIN']
STORE = os.environ['SEMANTIC_STORE']
CDB   = os.environ['C_DB']
CPROJ = os.environ.get('C_PROJECT', 'obrain-engine')
REPO  = os.environ.get('REPO_ROOT', os.getcwd())
DOMAIN= os.environ.get('DOMAIN', 'crates/obrain-engine/')

def bridge(query):
    payload = json.dumps({"tool": "query_graph",
                          "args": {"query": query, "language": "cypher"}})
    p = subprocess.run([BIN, "bridge"], input=payload + "\n",
                       capture_output=True, text=True, env={**os.environ, "OBRAIN_STORE": STORE})
    if p.returncode != 0:
        raise RuntimeError(f"bridge failed: {p.stderr[:400]}")
    out = p.stdout
    # la sortie contient une ligne 'obrain-mcp: store ouvert...' avant le JSON
    idx = out.find('{"content"')
    if idx < 0:
        raise RuntimeError("no JSON from bridge: " + out[:300])
    d = json.loads(out[idx:])
    return d["structuredContent"]["rows"]

# ---------- M6 : nœuds Function/Method + arêtes CALLS (pagination 10k) ----------
def m6_key(label, qn, name, fp):
    rel = fp.replace(REPO + "/", "")
    if label == "Method":
        seg = qn.split(".")
        return (rel, label, seg[-2] if len(seg) >= 2 else "", name)
    return (rel, label, "", name)

m_rows = bridge('MATCH (n) WHERE labels(n)[0] IN ["Function","Method"] RETURN labels(n)[0], n.qualified_name, n.name, n.file_path')
m_qn2key, m_key2node = {}, {}
for label, qn, name, fp in m_rows:
    k = m6_key(label, qn, name, fp)
    m_qn2key[qn] = k
    m_key2node[k] = (name, fp.replace(REPO + "/", ""))
m_edges_all, m_edges = [], set()
off = 0
while True:
    page = bridge(f'MATCH (a)-[r:CALLS]->(b) RETURN a.qualified_name, b.qualified_name ORDER BY a.qualified_name, b.qualified_name SKIP {off} LIMIT 10000')
    if not page:
        break
    m_edges_all.extend((a, b) for a, b in page)
    off += len(page)
    if len(page) < 10000:
        break
m_dropped_pairs = 0
for a, b in m_edges_all:
    ka, kb = m_qn2key.get(a), m_qn2key.get(b)
    if ka and kb:
        m_edges.add((ka, kb))
    else:
        m_dropped_pairs += 1
m_unique = set(m_edges_all)

# ---------- C (codebase-memory-mcp, SQLite) ----------
def c_key(label, name, qn, fp):
    if label == "Method":
        seg = qn.rstrip("#").split(".")
        return (fp, label, seg[-2] if len(seg) >= 2 else "", name)
    return (fp, label, "", name)

db = sqlite3.connect(f"file:{CDB}?mode=ro", uri=True)
cur = db.cursor()
c_qn2key, c_key2node = {}, {}
for label, name, qn, fp in cur.execute(
    "SELECT label,name,qualified_name,file_path FROM nodes WHERE project=? AND label IN ('Function','Method') AND file_path LIKE ?",
    (CPROJ, DOMAIN + "%")):
    k = c_key(label, name, qn, fp)
    c_qn2key[qn] = k
    c_key2node[k] = (name, fp)
c_rows = cur.execute(
    """SELECT sn.label,sn.name,sn.qualified_name,sn.file_path,
              tn.label,tn.name,tn.qualified_name,tn.file_path,e.properties
       FROM edges e JOIN nodes sn ON sn.id=e.source_id JOIN nodes tn ON tn.id=e.target_id
       WHERE e.type='CALLS' AND sn.label IN ('Function','Method') AND tn.label IN ('Function','Method')
         AND sn.project=? AND tn.project=?
         AND sn.file_path LIKE ? AND tn.file_path LIKE ?""",
    (CPROJ, CPROJ, DOMAIN + "%", DOMAIN + "%")).fetchall()
c_edges, c_strat = set(), {}
for sl, sn, sqn, sf, tl, tn, tqn, tf, props in c_rows:
    try:
        p = json.loads(props) if props else {}
    except Exception:
        p = {}
    ka, kb = c_qn2key.get(sqn), c_qn2key.get(tqn)
    if ka and kb:
        c_edges.add((ka, kb))
        c_strat.setdefault((ka, kb), set()).add(p.get("strategy", "?"))

def pct(x, y):
    return f"{100.0*x/y:.2f}" if y else "n/a"

inter = m_edges & c_edges
inv   = m_edges - inter
miss  = c_edges - inter
m_nodes = set(m_qn2key.values())
c_nodes = set(c_qn2key.values())

def short(k):
    fp, label, typ, name = k
    f = fp.replace(DOMAIN, "")
    return f"{name}@{f}" + (f"<{typ}>" if typ else "")

print("== périmètre ==")
print(f"M6 nodes (Function/Method) : {len(m_nodes)}")
print(f"C  nodes (Function/Method, {DOMAIN}) : {len(c_nodes)}")
print(f"nœuds communs (file,label,type,name) : {len(m_nodes & c_nodes)}")
print(f"M6 arêtes CALLS brutes : {len(m_unique)} paires uniques (dont {len(m_unique) - len(m_edges)} vers nœuds non-fonction)")
print(f"M6 arêtes fonctionnelles uniques : {len(m_edges)}")
print(f"C  arêtes CALLS fonctionnelles uniques (domaine {DOMAIN}) : {len(c_edges)}")
print()
print("== métriques (M6 vs C, identité (file,label,type,name)) ==")
print(f"|M6|={len(m_edges)} |C|={len(c_edges)} |M6∩C|={len(inter)}")
print(f"précision (M6∩C / M6) : {pct(len(inter), len(m_edges))}%")
print(f"rappel    (M6∩C / C ) : {pct(len(inter), len(c_edges))}%")
print(f"inventées (M6\\C) : {len(inv)}  dont cible absente du domaine C : {sum(1 for a,b in inv if b not in c_nodes)}")
print(f"manquées  (C\\M6) : {len(miss)} dont cible absente du domaine M6 : {sum(1 for a,b in miss if b not in m_nodes)}")
print()
print("== exemples inventées (dans M6, pas C ; cible connue de C) ==")
inv_ex = sorted((e[0], e[1]) for e in inv if e[1] in c_nodes)[:3]
for a, b in inv_ex:
    print(f"  {short(a)} -> {short(b)}")
print("== exemples manquées (dans C, pas M6 ; M6 possède les 2 nœuds) ==")
miss_ex = sorted((e[0], e[1]) for e in miss if e[0] in m_nodes and e[1] in m_nodes)[:3]
for a, b in miss_ex:
    st = ",".join(sorted(c_strat.get((a, b), {"?"})))
    print(f"  {short(a)} -> {short(b)}   [C strat: {st}]")
print()
# Référence C restreinte aux stratégies LSP (conf 0.85-0.95) : isole le bruit
# heuristique de C (suffix_match, unique_name...) du vrai manque de M6.
lsp_edges = {e for e in c_edges if any(s.startswith("lsp") for s in c_strat.get(e, []))}
inter_lsp = m_edges & lsp_edges
print(f"[réf.] C-LSP seul : |C_lsp|={len(lsp_edges)} |M6∩C_lsp|={len(inter_lsp)} "
      f"rappel M6/C_lsp={pct(len(inter_lsp), len(lsp_edges))}% "
      f"précision={pct(len(inter_lsp), len(m_edges))}%")
print(f"[réf.] C hors LSP (heuristiques, conf<0.85) : {len(c_edges - lsp_edges)} arêtes")
print()
print(f"RÉSUMÉ  precision {pct(len(inter), len(m_edges))}% recall {pct(len(inter), len(c_edges))}% edges M6/C/∩ = {len(m_edges)}/{len(c_edges)}/{len(inter)}")
print()
# Verdict de garde : seuils en fraction, défaut 0.5 — même contrat que l'appel
# depuis guard.sh (étape « résolution sémantique »). Mettre à 0 pour mesurer seul.
min_p = float(os.environ.get("SEMANTIC_MIN_PRECISION", "0.5"))
min_r = float(os.environ.get("SEMANTIC_MIN_RECALL", "0.5"))
p_ok = len(inter) >= min_p * len(m_edges)
r_ok = len(inter) >= min_r * len(c_edges)
print(f"GARDIEN résolution sémantique : precision {pct(len(inter), len(m_edges))}% "
      f"(seuil {min_p*100:.0f}%) recall {pct(len(inter), len(c_edges))}% (seuil {min_r*100:.0f}%)")
if p_ok and r_ok:
    print("GARDIEN : OK résolution sémantique")
    sys.exit(0)
else:
    print("GARDIEN : ÉCHEC résolution sémantique (precision ou recall sous le seuil)")
    sys.exit(1)
PYEOF
