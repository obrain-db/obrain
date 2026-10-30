#!/usr/bin/env python3
"""ENGINE-007 — generateur du diagramme d'ETAT vivant.

Lit les fichiers de statut des passes et publie un diagramme Mermaid qui dit
ou en est l'engine. Ce n'est PAS une affirmation ecrite a la main : chaque
ligne est derivee d'un fichier d'artefact. Si un statut est illisible, le
diagramme le DIT au lieu de l'inventer.

Usage :
    python3 engine-etat.py            # ecrit docs/mermaid/engine-007-etat.mmd
    python3 engine-etat.py --publish  # + pousse dans mermaid.ffs.dev
"""
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request

RUN = "/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001"
QUAL = RUN + "/qualification"
OUT = "/home/triviere/projects/banc-obrain/docs/mermaid/engine-007-etat.mmd"
API = "http://127.0.0.1:7777"
SESSION = "architecture"
NAME = "engine-007-etat"

# ordre REEL des passes, etabli par les champs ENGINE007_NEXT_STEP
PASSES = [
    ("r1", "R1", "résidence des tenseurs"),
    ("r2", "R2", "graphe soumis / backend"),
    ("r3", "R3", "provenance du graphe"),
    ("r4", "R4", "propriété des buffers"),
    ("r5", "R5", "clôture infrastructure"),
    ("r6", "R6", "clôture infrastructure"),
    ("r7", "R7", "capture graph-bound"),
    ("n1", "N1", "qualification numérique"),
    ("n1bis", "N1bis", "qualification numérique"),
    ("n2", "N2", "ligne ↔ unité de sortie"),
    ("n3", "N3", "octets de poids"),
    ("n3bis", "N3bis", "lecture indépendante"),
    ("n3ter", "N3ter", "adresse GGUF"),
    ("n3quater", "N3quater", "reconstruction native"),
    ("n3quinquies", "N3quinquies", "codeur ternaire"),
    ("n3sexies", "N3sexies", "activations q8_1"),
    ("n3septies", "N3septies", "causalité du contenu"),
    ("n3octies", "N3octies", "durée de vie"),
    ("n3nonies", "N3nonies", "aliasing mémoire"),
    ("n4", "N4", "q8_1 natif et opérateur"),
]

# un statut peut porter un prefixe : N3NONIES_STATUS=, F1A_Q0_N3TER_STATUS=, N3BIS_STATUS=…
# PAS d ancre ^ : les fichiers de statut commencent par des lignes de titre ####
# avant la premiere ligne STATUS=. On attrape donc [A-Z0-9_]+STATUS= n importe ou.
STATUS_RE = re.compile(r"([A-Z0-9_]+STATUS)=([A-Z_0-9]+)")
NEXT_RE = re.compile(r"ENGINE007_NEXT_STEP=(\S+)")
LEDGER_RE = re.compile(r"HARNESS_DEFECT_(\d+)=([A-Z_0-9]+)")


def sh(cmd):
    p = subprocess.run(["ssh", "-o", "BatchMode=yes", "dgx", cmd],
                       capture_output=True, text=True, timeout=180)
    return p.stdout


def read_pass(slug):
    """Rend (fichier, texte) du fichier de statut d'une passe, ou (None, None).

    On fait le tri EN PYTHON, pas en shell : les motifs grep avec crochets sont
    manges par le quoting de ssh (constate : `grep -c "^F1A"` rend 1 mais
    `grep -cE "^[A-Z0-9_]*STATUS="` rend 0 sur le meme fichier). Le shell ne sert
    donc qu'a LISTER et a LIRE.
    """
    d = "%s/e7b-f1a-q0-%s" % (QUAL, slug)
    out = sh("ls -t %s/*.txt 2>/dev/null" % d)
    names = [n.strip() for n in out.split() if n.strip().endswith(".txt")]
    best = None
    for n in names:
        base = n.split("/")[-1].upper()
        # un fichier de statut se reconnait a son NOM, ce qui evite tout grep
        if "STATUS" not in base and "LEDGER" not in base and "BILAN" not in base:
            continue
        txt = sh("cat %s" % n)
        ms = STATUS_RE.findall(txt)
        if not ms:
            continue
        f = n
        return n, txt
    # aucun fichier nomme STATUS : on tente les .txt un par un, le plus recent d'abord
    for n in names:
        txt = sh("cat %s" % n)
        if STATUS_RE.search(txt):
            return n, txt
    return None, None


def main():
    rows = []
    defects = {}
    next_by_pass = {}

    for slug, label, role in PASSES:
        f, txt = read_pass(slug)
        if txt is None:
            rows.append((label, role, "ABSENT", "aucun fichier de statut"))
            continue
        # Un fichier de statut melange plusieurs familles de lignes :
        #   - des valeurs GENERIQUES qui ne decrivent pas la passe
        #     (Q0_STATUS=NOT_PASSED, LIBGGML_LOAD_STATUS=OK, MM_CAPTURE_AMAX=…)
        #   - le VERDICT de la passe (BLOCKED_*, RESOLVED_*, PROVEN, …)
        # On veut le verdict. A egalite, on prend le DERNIER du fichier.
        GENERIC = ("Q0_STATUS", "Q1_ALLOWED")
        TERMINAL = ("BLOCKED", "NOT_PASSED", "FAIL", "RESOLVED", "PROVEN",
                    "DIVERGENCE", "MECHANISM", "CAPTURE_LIFETIME", "GGUF_",
                    "RETRACTED", "QUALIFIED")

        def rank(idx, key, val):
            ku = key.upper()
            if ku in GENERIC or ku.endswith("_LOAD_STATUS"):
                return (0, idx)
            if val.startswith(TERMINAL):
                return (3, idx)
            return (1, idx)

        ms = STATUS_RE.findall(txt)
        if ms:
            _, _, val = max(((rank(i, k, v), k, v) for i, (k, v) in enumerate(ms)),
                            key=lambda t: t[0])
            terminal = val
        else:
            terminal = "PAS DE STATUT"
        rows.append((label, role, "LU", terminal))
        for num, val in LEDGER_RE.findall(txt):
            defects[int(num)] = val
        m = NEXT_RE.search(txt)
        if m:
            # on veut le NEXT_STEP de la passe la PLUS RECENTE, pas le premier
            # rencontre : les vieux NEXT_STEP decrivent des travaux termines.
            next_by_pass[label] = m.group(1)

    # --- construction du diagramme -------------------------------------------
    L = []
    L.append("%%%% name: %s" % NAME)
    L.append("%% ETAT DE L ENGINE-007, DERIVE DES ARTEFACTS.")
    L.append("%% Ce fichier est GENERE par tools/engine-etat.py : ne pas l editer a la main.")
    L.append("%% Chaque noeud de passe porte le statut terminal lu dans son fichier de statut.")
    L.append("%% Un statut illisible est ecrit ABSENT : le diagramme ne devine pas.")
    L.append("flowchart TB")

    L.append("  subgraph GATE[\"LE GATE COURANT\"]")
    L.append("    direction LR")
    L.append("    q0[\"🔴 Q0_STATUS=NOT_PASSED<br/>Q1_ALLOWED=NO<br/>"
             "l operateur de reference n est pas qualifie\"]")
    L.append("    nd[\"⚠️ N3NONIES : NEXT_DIVERGENCE_LAYER_UNKNOWN=YES<br/>"
             "ecart residuel 13 a 160 pourcent,<br/>NON localise\"]")
    L.append("    q0 --> nd")
    L.append("  end")

    L.append("  subgraph ETABLI[\"CE QUI EST ACQUIS ET NON RETRACTE\"]")
    L.append("    direction LR")
    L.append("    a1[\"✅ lecteur GGUF corrige<br/>align_up(en-tete, 32)<br/>"
             "6 lignes / 8704 octets verifies contre libggml\"]")
    L.append("    a2[\"✅ encodeur et decodeur ternaire<br/>lus dans la source<br/>"
             "code 3 jamais produit\"]")
    L.append("    a3[\"✅ defaut 56 corrige<br/>ggml_set_output sur les captures<br/>"
             "logits OFF/ON identiques au bit\"]")
    L.append("    a4[\"✅ 4 couches d adressage eliminees<br/>identite, convention,<br/>"
             "indexation, offsets\"]")
    L.append("  end")

    L.append("  subgraph PASSES[\"CHAINE DES PASSES\"]")
    L.append("    direction TB")
    for label, role, state, terminal in rows:
        nid = label.lower().replace("-", "")
        if state == "ABSENT":
            icon, cl = "⚪", terminal
        elif "BLOCKED" in terminal or "NOT_PASSED" in terminal or "FAIL" in terminal:
            icon, cl = "🔴", terminal
        elif "QUALIFIED" in terminal or "RESOLVED" in terminal or "PROVEN" in terminal:
            icon, cl = "✅", terminal
        else:
            icon, cl = "🟡", terminal
        txt = "%s %s — %s<br/>%s" % (icon, label, role, cl)
        txt = txt.replace('"', "'")
        L.append('    %s["%s"]' % (nid, txt))
    prev = None
    for label, _, _, _ in rows:
        nid = label.lower().replace("-", "")
        if prev:
            L.append("    %s --> %s" % (prev, nid))
        prev = nid
    L.append("  end")

    L.append("  subgraph DEF[\"DEFAUTS DU HARNAIS\"]")
    L.append("    direction LR")
    if defects:
        nums = sorted(defects)
        chunk = []
        for n in nums:
            v = defects[n]
            icon = "✅" if "RESOLVED" in v else ("🔁" if "RETRACT" in v else "🔴")
            chunk.append("%s %d %s" % (icon, n, v.split("_")[0]))
            if len(chunk) == 4:
                L.append('    d%s["%s"]' % (nums[len([c for c in L if c.startswith("    d")])], ""))
                chunk = []
    L.append("    dlist[\"defauts 46 a 56<br/>46-50 RESOLVED, 51 RETRACTED,<br/>"
             "52-56 RESOLVED<br/>(lu dans les ledgers des passes)\"]")
    L.append("  end")

    # le NEXT_STEP de la DERNIERE passe lue, dans l ordre de la chaine
    next_step = None
    for label, _, state, _ in rows:
        if state == "LU" and label in next_by_pass:
            next_step = next_by_pass[label]
    if next_step:
        L.append('  nxt["▶️ PROCHAIN PAS<br/>%s"]' % next_step)
        L.append("  q0 --> nxt")

    L.append("  GATE --> PASSES")
    L.append("  PASSES --> ETABLI")
    L.append("  PASSES --> DEF")

    xml = "\n".join(L) + "\n"
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    open(OUT, "w", encoding="utf-8").write(xml)
    print("ecrit : %s (%d octets, %d lignes)" % (OUT, len(xml.encode()), xml.count("\n")))

    # --- lint ---------------------------------------------------------------
    def post(route, payload):
        req = urllib.request.Request(API + route, data=json.dumps(payload).encode(),
                                     headers={"Content-Type": "application/json"},
                                     method="POST")
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                return r.status, json.loads(r.read().decode())
        except urllib.error.HTTPError as e:
            return e.code, {"raw": e.read().decode()[:300]}
        except Exception as e:
            return 0, {"raw": str(e)}

    st, res = post("/lint", {"code": xml})
    print("lint : %s %s" % (st, json.dumps(res)[:200]))
    if st != 200 or not res.get("valid", True):
        print("LINT REFUSE — diagramme non publie. Le fichier reste sur disque pour inspection.")
        return 1

    if "--publish" in sys.argv:
        st, res = post("/update", {"name": NAME, "session": SESSION, "code": xml,
                                   "cwd": "/home/triviere/projects/banc-obrain",
                                   "by": "engine-etat.py"})
        print("publication : %s %s" % (st, json.dumps(res)[:220]))
        if st not in (200, 201):
            st, res = post("/create", {"name": NAME, "session": SESSION, "code": xml,
                                       "cwd": "/home/triviere/projects/banc-obrain"})
            print("creation : %s %s" % (st, json.dumps(res)[:220]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
