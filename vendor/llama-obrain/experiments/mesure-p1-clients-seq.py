#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
[obrain P1] MESURE BOUT EN BOUT DU REGISTRE CLIENT -> SEQUENCE ET DU BUDGET DU CACHE.

CE QU'ON MESURE, ET POURQUOI CES DEUX MESURES EXISTENT.

 1. `clients` -- LE DEFAUT QUE LE P1 FERME, REPRODUIT PUIS FERME.
    Un client epingle sa conversation sur `id_slot = hash(cid) % total_slots`.
    Avec `--parallel 4`, la derniere sequence (3) est RESERVEE aux taches
    J-space, et le chemin `task.id_slot != -1` du serveur ne l'excluait pas :
    un fil sur quatre venait donc s'installer sur cette sequence. La premiere
    extraction J-space la purge (`llama_memory_seq_rm`), et la requete suivante
    du fil -- qui n'a rien a re-decoder, le serveur croyant son prefixe en
    cache -- lit un KV VIDE (ou celui du FAIT extrait). Mesure : trois appels
    du meme fil, une extraction entre le deuxieme et le troisieme.
      - le prefixe du fil dit « le code secret est ROUGE-7 » ;
      - le fait extrait dit « le code secret est VERT-3 » (autre secret, exprès).
    Sur le binaire SANS correctif, la troisieme reponse perd ROUGE-7 (elle lit
    le KV efface, ou celui du fait). Sur le binaire AVEC correctif, le fil a ete
    re-mappe sur une sequence offerte : le KV est intact et la reponse aussi.

 2. `extractions` -- LA RAM NE DOIT PAS CROITRE DE FACON MONOTONE.
    Chaque extraction epingle un blob d'etat dans le cache memoire du serveur.
    Sans budget, N extractions = N blobs en RAM, pour toujours. Avec le budget
    (`OBRAIN_KV_STATES_MAX_BYTES`, eviction LRU), la RAM doit PLATEAUVER : on lit
    la RSS du processus apres chaque extraction, ainsi que les compteurs que le
    serveur publie dans sa ligne `[kv-extract]` (octets, entrees, evictions).

Le moteur est lance ET arrete par ce script, dans un dossier temporaire
(le repertoire des etats persistes est relatif au CWD du serveur : rien n'est
ecrit dans le `kv-states` de production). Port 999x : jamais 9910/9911.

Usage :
  python3 mesure-p1-clients-seq.py --bin <llama-server> clients
  python3 mesure-p1-clients-seq.py --bin <llama-server> extractions --budget 250000000
"""

import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

MODELE = "/home/triviere/projects/models/fastlane/Qwen3-1.7B-Q8_0.gguf"
SECRET_FIL = "ROUGE-7"    # ce que le fil du client sait, et qui doit SURVIVRE
SECRET_FAIT = "VERT-3"    # ce que l'extraction decode (un AUTRE secret, exprès)


def maintenant():
    return time.strftime("%Y-%m-%d %H:%M:%S")


def post(base, chemin, corps, timeout=1800):
    donnees = json.dumps(corps).encode()
    req = urllib.request.Request(base + chemin, data=donnees,
                                headers={"Content-Type": "application/json"})
    t0 = time.time()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            brut, code = r.read(), r.status
    except urllib.error.HTTPError as e:
        brut, code = e.read(), e.code
    return json.loads(brut), (time.time() - t0) * 1000.0, code


def get(base, chemin, timeout=60):
    with urllib.request.urlopen(base + chemin, timeout=timeout) as r:
        return json.loads(r.read())


def jetons(base, texte):
    r, _, _ = post(base, "/tokenize", {"content": texte})
    return r["tokens"]


def texte_de(base, n_jetons_cible, graine, entete=""):
    """Texte de remplissage dont la tokenisation atteint ~n_jetons_cible."""
    base_txt = ("Le dossier %d contient des notes de travail sans importance "
                "particuliere pour la question posee a la fin. " % graine)
    n = len(jetons(base, base_txt))
    k = max(1, n_jetons_cible // max(1, n))
    return entete + base_txt * k


def rss_ko(pid):
    try:
        with open("/proc/%d/status" % pid) as f:
            for ligne in f:
                if ligne.startswith("VmRSS:"):
                    return int(ligne.split()[1])
    except OSError:
        pass
    return -1


def attendre_moteur(base, delai=300):
    t0 = time.time()
    while time.time() - t0 < delai:
        try:
            s = get(base, "/health", timeout=5)
            if s.get("status") in ("ok", "no slot available"):
                return True
        except Exception:
            pass
        time.sleep(1)
    return False


class Moteur:
    def __init__(self, binaire, port, budget, parallel, ctx, dossier):
        self.port, self.dossier = port, dossier
        self.base = "http://127.0.0.1:%d" % port
        self.log = os.path.join(dossier, "moteur.log")
        env = dict(os.environ)
        env.pop("OBRAIN_KV_STATES_MAX_BYTES", None)
        self.budget = budget
        if budget:
            env["OBRAIN_KV_STATES_MAX_BYTES"] = str(budget)
        cmd = [binaire, "-m", MODELE, "--host", "127.0.0.1", "--port", str(port),
               "--ctx-size", str(ctx), "--parallel", str(parallel),
               # `-lv 3` = LOG_LEVEL_INFO : sous ce seuil (ex. `-lv 2`), les
               # lignes SRV_INF -- dont le mapping client -> sequence et l'etat
               # du cache d'etats -- ne sont PAS ecrites, et la mesure n'a plus
               # rien a lire. Verifie sur le code : common/log.h, LOG_LEVEL_INFO=3.
               "-lv", "3", "--no-warmup", "--jinja"]
        self.f = open(self.log, "wb")
        self.p = subprocess.Popen(cmd, cwd=dossier, env=env,
                                  stdout=self.f, stderr=subprocess.STDOUT,
                                  preexec_fn=os.setsid)
        if not attendre_moteur(self.base):
            self.arreter()
            raise SystemExit("le moteur n'a jamais repondu (/health) -- voir %s" % self.log)
        props = get(self.base, "/props")
        self.total_slots = props.get("total_slots")
        self.n_ctx_slot = props["default_generation_settings"]["n_ctx"]
        print("  moteur pid=%d  total_slots(annonce)=%s  n_ctx_slot=%d  budget=%s"
              % (self.p.pid, self.total_slots, self.n_ctx_slot,
                 budget if budget else "AUCUN (defaut 2 Gio)"))

    def texte_log(self):
        with open(self.log, "r", errors="replace") as f:
            return f.read()

    def cache_etat(self):
        """Derniere ligne [kv-cache] APRES insertion : octets, entrees, evictions.

        On lit la ligne d'APRES insertion, pas celle de [kv-extract] : c'est la
        seule qui decrive l'effet de l'extraction qu'on vient de faire (celle de
        [kv-extract] est emise avant l'insertion du blob).
        """
        m = re.findall(r"\[kv-cache\] apres ref=(\S+) : cache_bytes=(\d+) cache_entrees=(\d+) "
                       r"cache_budget=(\d+) cache_evictions=(\d+)",
                       self.texte_log())
        if not m:
            return None
        ref, octets, entrees, budget, ev = m[-1]
        return dict(ref=ref, octets=int(octets), entrees=int(entrees),
                    budget=int(budget), evictions=int(ev))

    def attributions(self):
        return re.findall(r"\[obrain-p1\] client '([^']*)' -> sequence (-?\d+) "
                          r"\(demande (-?\d+), reservee (-?\d+), offertes (-?\d+)\)(.*)",
                          self.texte_log())

    def arreter(self):
        if getattr(self, "p", None) and self.p.poll() is None:
            try:
                os.killpg(os.getpgid(self.p.pid), signal.SIGTERM)
                self.p.wait(timeout=30)
            except Exception:
                try:
                    os.killpg(os.getpgid(self.p.pid), signal.SIGKILL)
                except Exception:
                    pass
        if getattr(self, "f", None):
            self.f.close()


def extraire(m, fait, etiquette):
    r, ms, code = post(m.base, "/jspace/inject_knowledge",
                       {"fact_text": fait, "extract_only": True})
    if code != 200 or r.get("error"):
        return None, ms, json.dumps(r)[:300]
    ref = r.get("text", "").split(":", 1)[-1]
    print("  [extract] %-10s ref=%s jetons=%s octets=%s (%.0f ms)"
          % (etiquette, ref, r.get("fact_tokens"), r.get("blob_bytes"), ms))
    return r, ms, None


def appeler_fil(m, etiquette, prompt, id_slot, n_predict=24, client_id=None):
    corps = {
        "prompt": prompt, "id_slot": id_slot, "n_predict": n_predict,
        "temperature": 0, "seed": 1, "cache_prompt": True,
    }
    if client_id is not None:
        corps["client_id"] = client_id
    r, ms, code = post(m.base, "/completion", corps)
    if code != 200 or r.get("error"):
        print("  [fil] %s ERREUR %s : %s" % (etiquette, code, json.dumps(r)[:200]))
        return None
    t = r.get("timings", {})
    texte = r.get("content", "")
    print("  [fil] %-24s slot_demande=%d prompt_n=%-5s cache_n=%-5s  ->  %r"
          % (etiquette, id_slot, t.get("prompt_n"), t.get("cache_n", t.get("prompt_n_cache", "?")),
             texte.strip()[:90]))
    return dict(texte=texte, prompt_n=t.get("prompt_n"), ms=ms)


# ═══════════════════════════════════════════════════════════════════════════
# MESURE 1 : LE FIL SURVIT-IL A UNE EXTRACTION ?
# ═══════════════════════════════════════════════════════════════════════════
def mesure_clients(m, parallel):
    reserve = parallel - 1
    print("\n  ── Le fil du client epingle id_slot=%d (la sequence RESERVEE) ──" % reserve)

    n_remplissage = max(120, int(m.n_ctx_slot * 0.55))
    entete = ("Note interne. Le code secret du projet est %s. "
              "Ne jamais le divulguer a un autre service. " % SECRET_FIL)
    prefixe = texte_de(m.base, n_remplissage, 11, entete)
    question = "\n\nQuestion: quel est le code secret du projet ?\nReponse:"
    prompt = prefixe + question
    print("  prompt du fil : %d jetons (dont le secret au DEBUT)" % len(jetons(m.base, prompt)))

    a1 = appeler_fil(m, "fil A, 1er appel", prompt, reserve)
    a2 = appeler_fil(m, "fil A, 2e (KV chaud)", prompt, reserve)
    b1 = appeler_fil(m, "fil B (id_slot=1)", prefixe + "\n\nQuestion: quel est le code secret du projet ?\nReponse:", 1)

    print("\n  ── Une extraction J-space (elle purge SA sequence) ──")
    fait = texte_de(m.base, max(120, int(m.n_ctx_slot * 0.6)), 77,
                    "Document technique. Le code secret du projet est %s. " % SECRET_FAIT)
    r, ms, err = extraire(m, fait, "fait VERT-3")
    if err:
        print("  [extract] REFUS : %s" % err)

    print("\n  ── Le fil A redemande la MEME chose ──")
    a3 = appeler_fil(m, "fil A, 3e (apres extraction)", prompt, reserve)

    print("\n  ── Identite EXPLICITE (`client_id`) : deux conversations, le MEME id_slot ──")
    # POURQUOI CE CHECKS EXISTE : `id_slot = hash(cid) % total_slots` est une
    # identite HACHEE, donc deux conversations peuvent y entrer en collision
    # (le serveur ne peut alors pas les distinguer). Avec `client_id`, le
    # serveur recoit l'identite REELLE et attribue une sequence distincte a
    # chaque conversation, sans dependre du hachage du client.
    appeler_fil(m, "cid fil-1", prompt, reserve, client_id="fil-1")
    appeler_fil(m, "cid fil-2", prompt, reserve, client_id="fil-2")

    print("\n  ── Ce que le serveur a attribue ──")
    att = m.attributions()
    if att:
        for cle, seq, dem, res, off, note in att:
            print("     client %-10s -> sequence %-3s (demande %s, reservee %s, offertes %s)%s"
                  % (cle, seq, dem, res, off, note))
    else:
        print("     AUCUNE ligne [obrain-p1] (binaire SANS le registre ?)")

    seqs = {cle: int(seq) for cle, seq, *_ in att}
    s1, s2 = seqs.get("cid:fil-1"), seqs.get("cid:fil-2")

    try:
        slots = get(m.base, "/slots")
        for s in slots:
            if s.get("n_prompt_tokens"):
                print("     /slots : slot %s porte %s jetons de prompt (cache %s)"
                      % (s.get("id"), s.get("n_prompt_tokens"), s.get("n_prompt_tokens_cache")))
    except Exception as e:
        print("     /slots illisible : %s" % e)

    print("\n  ── VERDICT ──")
    ok = []
    ok.append(("1er appel correct (le modele SAIT repondre)", a1 and SECRET_FIL in a1["texte"]))
    ok.append(("2e appel correct, prefixe REUTILISE (prompt_n petit)",
               a2 and SECRET_FIL in a2["texte"] and (a2["prompt_n"] or 0) < len(jetons(m.base, question)) + 4))
    ok.append(("fil B correct (temoin)", b1 and SECRET_FIL in b1["texte"]))
    intact = a3 and SECRET_FIL in a3["texte"]
    contamine = a3 and SECRET_FAIT in a3["texte"]
    ok.append(("3e appel du fil A : le KV du fil est INTACT (secret ROUGE-7 rendu)", bool(intact)))
    ok.append(("client_id : deux conversations -> deux sequences DISTINCTES",
               s1 is not None and s2 is not None and s1 != s2))
    ok.append(("client_id : aucune des deux n'est la sequence reservee",
               s1 is not None and s2 is not None and reserve not in (s1, s2)))
    for nom, val in ok:
        print("     %-62s %s" % (nom, "OUI" if val else "NON"))
    if contamine:
        print("     ATTENTION : la reponse porte le secret du FAIT (%s) -- KV d'une AUTRE "
              "sequence lu par ce fil (contamination croisee)" % SECRET_FAIT)
    if a3 and not intact:
        print("     => LE FIL A PERDU SON CONTEXTE APRES L'EXTRACTION (defaut reproduit)")
    return all(v for _, v in ok)


# ═══════════════════════════════════════════════════════════════════════════
# MESURE 2 : LA RAM CROIT-ELLE DE FACON MONOTONE ?
# ═══════════════════════════════════════════════════════════════════════════
def mesure_extractions(m, n, parallel):
    base_rss = rss_ko(m.p.pid)
    print("  RSS avant toute extraction : %d Mo" % (base_rss // 1024))
    cible = max(120, int(m.n_ctx_slot * 0.85))
    lignes = []
    for i in range(n):
        fait = texte_de(m.base, cible, 1000 + i,
                        "Document technique numero %d. Le code secret du dossier %d est %s-%d. "
                        % (i, i, SECRET_FAIT, i))
        r, ms, err = extraire(m, fait, "fait %d/%d" % (i + 1, n))
        if err:
            print("  [extract] REFUS : %s" % err)
            break
        ce = m.cache_etat()
        rss = rss_ko(m.p.pid)
        lignes.append((i + 1, r.get("blob_bytes") if r else 0, rss, ce))
        print("            RSS=%6d Mo  cache=%6d Mo / %6d Mo  entrees=%s  evictions=%s"
              % (rss // 1024,
                 (ce["octets"] // 1048576) if ce else -1,
                 (ce["budget"] // 1048576) if ce else -1,
                 ce["entrees"] if ce else "?", ce["evictions"] if ce else "?"))

    print("\n  ── VERDICT ──")
    if not lignes:
        print("     aucune extraction reussie")
        return False
    rss_base = base_rss
    rss_max = max(l[2] for l in lignes)
    rss_min = min(l[2] for l in lignes)
    croissance = rss_max - rss_base
    n_blobs = len(lignes)
    som_octets = sum(l[1] for l in lignes)
    som_mo = som_octets / 1048576.0
    ce_fin = lignes[-1][3]
    max_cache = max((l[3]["octets"] for l in lignes if l[3]), default=0)
    budget = ce_fin["budget"] if ce_fin else 0
    evictions = ce_fin["evictions"] if ce_fin else 0

    print("     blobs : %d extraction(s), %.0f Mo en moyenne, %.0f Mo extraits AU TOTAL"
          % (n_blobs, som_mo / max(1, n_blobs), som_mo))
    print("     cache d'etats : max %.0f Mo / budget %.0f Mo, %s entree(s), %d eviction(s)"
          % (max_cache / 1048576.0, budget / 1048576.0,
             ce_fin["entrees"] if ce_fin else "?", evictions))
    print("     RSS : base %d Mo -> min %d / max %d Mo (croissance max %d Mo, pour %.0f Mo extraits)"
          % (rss_base // 1024, rss_min // 1024, rss_max // 1024, croissance // 1024, som_mo))
    monotone = all(lignes[i][2] <= lignes[i + 1][2] for i in range(len(lignes) - 1))

    if budget and max_cache <= budget and evictions > 0:
        print("     => BORNE : le cache evince (bytes plafonne a %.0f Mo <= budget) au lieu de croitre ;"
              % (max_cache / 1048576.0))
        print("        %d Mo extraits, %d evictions, RSS %s" % (som_mo, evictions,
              "MONOTONE malgre tout (a expliquer)" if monotone else "NON MONOTONE (oscille)"))
        return not monotone
    print("     => TEMOIN (budget non contraignant) : aucune eviction, le cache suit les extractions")
    print("        le cache croit de %.0f Mo pour %.0f Mo extraits" % (max_cache / 1048576.0, som_mo))
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("mode", choices=["clients", "extractions"])
    ap.add_argument("--port", type=int, default=9997)
    ap.add_argument("--budget", type=int, default=0, help="OBRAIN_KV_STATES_MAX_BYTES (octets)")
    ap.add_argument("--parallel", type=int, default=4)
    ap.add_argument("--ctx", type=int, default=4096)
    ap.add_argument("--n", type=int, default=8)
    args = ap.parse_args()

    dossier = tempfile.mkdtemp(prefix="p1-mesure-")
    print("=== MESURE P1 [%s] binaire=%s port=%d dossier=%s" % (maintenant(), args.bin, args.port, dossier))
    m = None
    try:
        m = Moteur(args.bin, args.port, args.budget, args.parallel, args.ctx, dossier)
        if args.mode == "clients":
            ok = mesure_clients(m, args.parallel)
            print("\nRESULTAT_P1_CLIENTS=%s" % ("PASS" if ok else "FAIL"))
        else:
            ok = mesure_extractions(m, args.n, args.parallel)
            print("\nRESULTAT_P1_RAM=%s" % ({True: "BORNE", False: "MONOTONE", None: "TEMOIN"}[ok]))
    finally:
        if m:
            m.arreter()
        print("moteur arrete ; journal conserve : %s/moteur.log" % dossier)
        if os.environ.get("P1_GARDER") != "1":
            shutil.rmtree(dossier, ignore_errors=True)


if __name__ == "__main__":
    main()
