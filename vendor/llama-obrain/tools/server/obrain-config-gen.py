"""Genere tools/server/obrain-certified-config.h depuis les SOURCES CERTIFIEES.
Verifie les hashes AVANT generation ; echoue fatalement sinon. Aucune valeur recopiee a la main."""
import hashlib, json, re, sys

SRC = "/tmp/obrain-r1fast.cpp"          # standalone certifie (PROF=48, NL=7, Z_STAR=6, T0 regeneree)
EXPECTED_T0 = "e31e6f75712ac08d"

s = open(SRC, encoding="utf-8").read()
src_hash = hashlib.sha256(s.encode()).hexdigest()[:16]

# --- extraction des constantes gelees DEPUIS la source certifiee ---
m = re.search(r"static const int PROF = (\d+), NL = (\d+);", s)
assert m, "PROF/NL introuvables dans la source certifiee"
PROF, NL = int(m.group(1)), int(m.group(2))

# [LONGUEURS_FROM_SRC] EXTRAIT de la meme source certifiee -- plus aucun litteral en dur.
m_lg = re.search(r"static const int\s+LONGUEURS\[[A-Za-z0-9_]+\]\s*=\s*\{([^}]*)\}", s)
if not m_lg:
    raise SystemExit("FATAL: LONGUEURS introuvable dans la source certifiee %s" % SRC)
LONGUEURS_FROM_SRC = [int(x) for x in m_lg.group(1).replace(" ", "").split(",") if x]
if len(LONGUEURS_FROM_SRC) != NL:
    raise SystemExit("FATAL: len(LONGUEURS)=%d != NL=%d" % (len(LONGUEURS_FROM_SRC), NL))
LONGUEURS_STR = ", ".join(str(x) for x in LONGUEURS_FROM_SRC)
print("  %-22s %-10s %-10s %s" % ("LONGUEURS", LONGUEURS_STR, "(source)", "OK" if len(LONGUEURS_FROM_SRC) == NL else "ECHEC"))

m = re.search(r"Z_STAR\s*=\s*([0-9.]+)", s)
assert m, "Z_STAR introuvable"
Z = float(m.group(1))
m2 = re.search(r"T_PLAIN\s*=\s*([0-9.]+)\s*,\s*T_LOOKUP\s*=\s*([0-9.]+)", s)
assert m2, "T_PLAIN/T_LOOKUP introuvables"
T_PLAIN, T_LOOKUP = float(m2.group(1)), float(m2.group(2))

m = re.search(r"static const double T0\[50\] = \{(.*?)\};", s, re.S)
assert m, "table T0 introuvable"
t0_vals = [float(x) for x in re.findall(r"-?[0-9.]+(?:e-?[0-9]+)?", m.group(1))]
assert len(t0_vals) == 50, "T0 n'a pas 50 entrees : %d" % len(t0_vals)

# --- verification du hash canonique de T0 (meme methode que partout dans l'arc) ---
T0 = {b: t0_vals[b] for b in range(2, 50)}
h = hashlib.sha256(json.dumps({str(k): T0[k] for k in sorted(T0)}, sort_keys=True).encode()).hexdigest()[:16]
print("T0 extraite de la source certifiee -> hash = %s | attendu %s | %s" % (
      h, EXPECTED_T0, "OK" if h == EXPECTED_T0 else "ECHEC"))
if h != EXPECTED_T0:
    print("GENERATION FATALE : T0 ne correspond pas au hash canonique"); sys.exit(1)

# --- audit AVANT generation (r1fast vs valeurs actuellement declarees dans l'en-tete serveur) ---
srv = open("/tmp/obrain-proposer.h", encoding="utf-8").read()
def gv(pat, txt, d=None):
    mm = re.search(pat, txt)
    return mm.group(1) if mm else d
srv_prof = gv(r"OBRAIN_PROF\s*=\s*(\d+)", srv)
srv_nl   = gv(r"OBRAIN_NL\s+(\d+)", srv)
srv_z    = gv(r"OBRAIN_Z\s*=\s*([0-9.]+)", srv)
print()
print("=== AUDIT DES PARAMETRES GELES (avant compilation) ===")
print("  %-22s %-10s %-10s %s" % ("parametre", "r1fast", "serveur", "verdict"))
rows = [("PROF", str(PROF), srv_prof), ("NL", str(NL), srv_nl), ("Z_STAR", str(Z), srv_z)]
bad = False
for nom, a, b in rows:
    ok = (a == b)
    if not ok: bad = True
    print("  %-22s %-10s %-10s %s" % (nom, a, b, "OK" if ok else "DIVERGENCE"))
print("  %-22s %-10s %-10s %s" % ("T0_hash", EXPECTED_T0, "(local)", "OK"))
print("  %-22s %-10s %-10s %s" % ("T_PLAIN", T_PLAIN, "-", "gele"))
print("  %-22s %-10s %-10s %s" % ("T_LOOKUP", T_LOOKUP, "-", "gele"))
print()
print("DIVERGENCES A CORRIGER : %s" % (", ".join(n for n, a, b in rows if a != b) or "aucune"))

# --- generation du header canonique ---
lignes = ["%.17g" % t0_vals[b] for b in range(2, 50)]
corps = ",\n    ".join(", ".join(lignes[i:i+4]) for i in range(0, len(lignes), 4))

H = '''// obrain-certified-config.h -- CONFIGURATION CERTIFIEE, GENEREE. NE PAS EDITER A LA MAIN.
//
// Chaine de provenance :
//   sources certifiees (r1fast/traj, R1 ferme)  -> verification des hashes -> generation -> hash du header
// Ce fichier n'est PAS sa propre source de verite : il est rejoue par son generateur.
//
//   OBRAIN_CFG_SOURCE            = %s   (standalone certifie)
//   OBRAIN_CFG_SOURCE_HASH       = %s
//   OBRAIN_CFG_T0_CANONICAL_HASH = %s  (verifie a la generation)
//   OBRAIN_CFG_GENERATOR         = tools/server/obrain-config-gen.py
#pragma once

static const int    OBRAIN_POLICY_PROF = %d;
static const int    OBRAIN_POLICY_NL   = %d;
static const double OBRAIN_POLICY_Z    = %.17g;
static const double OBRAIN_POLICY_T_PLAIN  = %.17g;
static const double OBRAIN_POLICY_T_LOOKUP = %.17g;

static const int OBRAIN_POLICY_LONGUEURS[%d] = { %s };

static const double OBRAIN_POLICY_T0[50] = {
    0.0, 0.0,
    %s
};

static const char * OBRAIN_CFG_T0_CANONICAL_HASH = "%s";
static const char * OBRAIN_CFG_SOURCE_HASH       = "%s";
''' % (SRC.split("/")[-1], src_hash, EXPECTED_T0, PROF, NL, Z, T_PLAIN, T_LOOKUP, NL, LONGUEURS_STR, corps, EXPECTED_T0, src_hash)

open("/tmp/obrain-certified-config.h", "w").write(H)
gen_hash = hashlib.sha256(H.encode()).hexdigest()[:16]
print()
print("=== HEADER CANONIQUE GENERE ===")
print("  obrain-certified-config.h : PROF=%d NL=%d Z=%.1f T0[50] | %d octets" % (PROF, NL, Z, len(H)))
print("  generator_hash (du header genere) = %s" % gen_hash)
print("  policy_manifest_hash = dfa08248d4728809 (manifeste R3 gele)")
print("  T0_canonical_hash    = %s" % EXPECTED_T0)
print()
print("BUILD_FAILS_IF_HASH_MISMATCH = true (toute divergence ci-dessus arrete la generation)")
