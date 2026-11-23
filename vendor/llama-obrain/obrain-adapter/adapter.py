"""obrain-adapter — client du pont obrain ↔ llama-server /jspace (C3, PoC).

La logique obrain reste 100 % Rust (bridge RO + CALL LECTURE) ; ce module ne
fait que du collage de validation : interroger obrain, composer les payloads
conformes aux schémas /jspace, poster (optionnel). Aucune logique moteur ici.
"""
from __future__ import annotations

import json
import subprocess
import urllib.request
from typing import Any, Dict, List, Optional


class ObrainBridgeRO:
    """Client minimal du bridge obrain en lecture seule (process spawné)."""

    def __init__(self, obrain_bin: str, store: str):
        self._bin = obrain_bin
        self._store = store
        self._p: Optional[subprocess.Popen] = None

    def _ensure(self) -> subprocess.Popen:
        if self._p is None or self._p.poll() is not None:
            self._p = subprocess.Popen(
                [self._bin, "bridge", "--store", self._store],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL, text=True, bufsize=1,
            )
        return self._p

    def query(self, gql: str, limit: int = 100) -> List[List[Any]]:
        """Exécute une requête GQL en lecture seule → lignes (liste de cellules)."""
        p = self._ensure()
        req = json.dumps({"tool": "query", "args": {"query": gql, "language": "gql",
                                                    "limit": limit}}) + "\n"
        p.stdin.write(req)          # type: ignore[union-attr]
        p.stdin.flush()             # type: ignore[union-attr]
        line = p.stdout.readline()  # type: ignore[union-attr]
        if not line:
            raise RuntimeError("bridge fermé sans réponse")
        d = json.loads(line)
        sc = d.get("structuredContent") or {}
        if d.get("isError") or "error" in sc:
            raise RuntimeError(f"requête refusée: {str(sc)[:200]}")
        return sc.get("rows", [])

    def close(self) -> None:
        if self._p is not None:
            try:
                self._p.stdin.close()  # type: ignore[union-attr]
                self._p.wait(timeout=10)
            except Exception:
                self._p.kill()  # type: ignore[union-attr]
            self._p = None


# ---------------------------------------------------------------------------
# Schémas /jspace (documentés dans README) — builders testables hors serveur
# ---------------------------------------------------------------------------

def payload_compare_generation(prompt: str, scale: float = 0.15,
                               n_generate: int = 24) -> Dict[str, Any]:
    return {"prompt": prompt, "scale": scale, "n_generate": n_generate}


def payload_inject_knowledge(fact_text: str, prompt: str,
                             n_generate: int = 24) -> Dict[str, Any]:
    return {"fact_text": fact_text, "prompt": prompt, "n_generate": n_generate}


def payload_readout_forward(h: List[float], n_embd: int) -> Dict[str, Any]:
    if len(h) != n_embd:
        raise ValueError(f"'h' length must be {n_embd}, got {len(h)}")
    return {"h": h}


def post_json(url: str, body: Dict[str, Any], timeout: float = 120.0) -> Dict[str, Any]:
    req = urllib.request.Request(
        url, data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"}, method="POST")
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode())


# ---------------------------------------------------------------------------
# Sélection de candidats depuis obrain (déterministe, petit graphe)
# ---------------------------------------------------------------------------

def top_candidates_surprise(bridge: ObrainBridgeRO, since_ms: int,
                            limit: int = 5) -> List[Dict[str, Any]]:
    """Top entités par surprise d'activité → {node_id, surprise, qn} si dispo."""
    rows = bridge.query(
        f"CALL obrain.search_surprise({since_ms}, {limit}, false) "
        "YIELD node_id, surprise RETURN node_id, surprise", limit=limit)
    out = []
    for r in rows:
        try:
            nid = int(r[0]); sur = float(r[1])
        except (TypeError, ValueError, IndexError):
            continue
        qn = None
        try:
            qr = bridge.query(f"MATCH (n) WHERE id(n) = {nid} "
                              "RETURN n.qualified_name LIMIT 1")
            if qr and qr[0]:
                qn = qr[0][0]
        except Exception:
            pass
        out.append({"node_id": nid, "surprise": sur, "qn": qn})
    return out


def fact_from_node(bridge: ObrainBridgeRO, node_id: int,
                   prop_keys: tuple = ("text", "name", "summary")) -> Optional[str]:
    """Extrait un « fait » texte d'un nœud (propriétés candidates)."""
    for k in prop_keys:
        try:
            rows = bridge.query(
                f"MATCH (n) WHERE id(n) = {node_id} RETURN n.{k} LIMIT 1")
            if rows and rows[0] and isinstance(rows[0][0], str) and rows[0][0]:
                txt = rows[0][0].strip()
                return txt if len(txt) <= 800 else txt[:800]
        except Exception:
            continue
    return None


def payload_extract_state(text: str) -> Dict[str, Any]:
    """K1 : extraire l'état K/V d'un texte → réponse {text: 'kv_state:<ref>', ...}."""
    return {"fact_text": text, "extract_only": True}


def payload_inject_state(state_ref: str, prompt: str, n_generate: int = 24) -> Dict[str, Any]:
    """K2 : injection ultra-rapide par état pré-calculé (bypass texte, zéro prefill)."""
    return {"state_ref": state_ref, "prompt": prompt, "n_generate": n_generate, "greedy": True}


def record_kv_state_ref(obrain_bin: str, store: str, qn: str, kv_ref: str) -> None:
    """K1b : enregistre le ref d'état K/V sur le nœud obrain (propriété kv_state_ref)."""
    q = f"MATCH (n {{qualified_name: '{qn}'}}) SET n.kv_state_ref = '{kv_ref}'"
    r = subprocess.run([obrain_bin, "query", store, q], capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"record kv_state_ref échoué: {r.stderr[:200]}")


def metrics_from_inject(resp: Dict[str, Any]) -> Dict[str, Any]:
    """Métriques directives (3) depuis une réponse d'injection/génération serveur.
    TFPT ≈ premier per_token_ms (ms) ; tok/s = 1000 / moyenne(per_token_ms)."""
    pt = resp.get("per_token_ms") or []
    tfpt_ms = float(pt[0]) if pt else None
    tok_per_s = (1000.0 / (sum(pt) / len(pt))) if pt else None
    qd = resp.get("query_decode_ms")
    fd = resp.get("fact_decode_ms")
    return {
        "tfpt_ms": tfpt_ms,
        "tokens_per_s": round(tok_per_s, 3) if tok_per_s else None,
        "query_decode_ms": qd,
        "fact_or_state_load_ms": fd,
        "n_tokens": len(pt),
    }
