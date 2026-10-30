#!/usr/bin/env python3
"""ENGINE-008 : lecture des metadonnees GGUF (sans donnees tenseur).
Audit d'identite logique baseline vs candidat."""
import struct, sys, json, hashlib

T_U8, T_I8, T_U16, T_I16, T_U32, T_I32, T_F32, T_BOOL, T_STR, T_ARR, T_U64, T_I64, T_F64 = range(13)
SCAL = {T_U8: ("<B", 1), T_I8: ("<b", 1), T_U16: ("<H", 2), T_I16: ("<h", 2),
        T_U32: ("<I", 4), T_I32: ("<i", 4), T_F32: ("<f", 4), T_BOOL: ("<?", 1),
        T_U64: ("<Q", 8), T_I64: ("<q", 8), T_F64: ("<d", 8)}

def read_str(f):
    n = struct.unpack("<Q", f.read(8))[0]
    return f.read(n).decode("utf-8", "replace")

def read_val(f, t):
    if t in SCAL:
        fmt, sz = SCAL[t]
        return struct.unpack(fmt, f.read(sz))[0]
    if t == T_STR:
        return read_str(f)
    if t == T_ARR:
        et = struct.unpack("<I", f.read(4))[0]
        n = struct.unpack("<Q", f.read(8))[0]
        # CORRECTIF : il faut TOUJOURS consommer les elements, sinon le flux
        # se desynchronise (tokenizer.ggml.tokens fait ~150k chaines).
        if et == T_STR:
            vals = [read_str(f) for _ in range(n)]
            return vals
        if et == T_ARR:
            for _ in range(n):
                read_val(f, T_ARR)
            return ["<array imbrique, %d sous-tableaux>" % n]
        fmt, sz = SCAL[et]
        # les grands tableaux numeriques (token_type, scores) sont lus en entier :
        # necessaire pour la coherence du flux et pour l audit d identite
        return [struct.unpack(fmt, f.read(sz))[0] for _ in range(n)]
    raise ValueError("type %d inconnu" % t)

def load_meta(path, max_bytes=32 * 1024 * 1024):
    meta = {}
    with open(path, "rb") as f:
        magic = f.read(4)
        assert magic == b"GGUF", magic
        ver = struct.unpack("<I", f.read(4))[0]
        n_tensors = struct.unpack("<Q", f.read(8))[0]
        n_kv = struct.unpack("<Q", f.read(8))[0]
        meta["__version"] = ver
        meta["__tensor_count"] = n_tensors
        read = 0
        for _ in range(n_kv):
            k = read_str(f)
            t = struct.unpack("<I", f.read(4))[0]
            meta[k] = read_val(f, t)
            read = f.tell()
            if read > max_bytes:
                meta["__TRUNCATED_AT"] = read
                break
    return meta

def summarize(m):
    return {
        "version": m.get("__version"),
        "tensor_count": m.get("__tensor_count"),
        "architecture": m.get("general.architecture"),
        "name": m.get("general.name"),
        "basename": m.get("general.basename"),
        "size_label": m.get("general.size_label"),
        "file_type": m.get("general.file_type"),
        "quantization_version": m.get("general.quantization_version"),
        "block_count": m.get("qwen35.block_count") or m.get("qwen3.block_count"),
        "embedding_length": m.get("qwen35.embedding_length") or m.get("qwen3.embedding_length"),
        "context_length": m.get("qwen35.context_length") or m.get("qwen3.context_length"),
        "vocab_size": m.get("qwen35.vocab_size") or m.get("qwen3.vocab_size"),
        "tokenizer_model": m.get("tokenizer.ggml.model"),
        "tokenizer_pre": m.get("tokenizer.ggml.pre"),
        "bos_id": m.get("tokenizer.ggml.bos_token_id"),
        "eos_id": m.get("tokenizer.ggml.eos_token_id"),
        "padding_id": m.get("tokenizer.ggml.padding_token_id"),
        "add_bos": m.get("tokenizer.ggml.add_bos_token"),
        "add_eos": m.get("tokenizer.ggml.add_eos_token"),
        "chat_template_present": "tokenizer.chat_template" in m,
        "rope_freq_base": m.get("qwen35.rope.freq_base") or m.get("qwen3.rope.freq_base"),
        "head_count": m.get("qwen35.attention.head_count") or m.get("qwen3.attention.head_count"),
        "head_count_kv": m.get("qwen35.attention.head_count_kv") or m.get("qwen3.attention.head_count_kv"),
    }

def tok_hash(m):
    toks = m.get("tokenizer.ggml.tokens")
    if not isinstance(toks, list): return None
    h = hashlib.sha256()
    for t in toks:
        h.update(t.encode("utf-8", "replace")); h.update(b"\x00")
    return h.hexdigest(), len(toks)

if __name__ == "__main__":
    base, cand = sys.argv[1], sys.argv[2]
    out = {}
    for label, path in (("baseline", base), ("candidate", cand)):
        m = load_meta(path)
        s = summarize(m)
        th = tok_hash(m)
        s["tokenizer_sha256"] = th[0] if th else None
        s["tokenizer_token_count"] = th[1] if th else None
        # types de tokens (normal vs special) : absents du dump tronque si trop gros
        for k in ("tokenizer.ggml.token_type", "tokenizer.ggml.merges"):
            if k in m and isinstance(m[k], list): s[k + "_count"] = len(m[k])
        out[label] = s
        print("=== %s ===" % label.upper())
        for k in sorted(s): print("  %-28s %s" % (k, s[k]))
        print()
    print("=== IDENTITE ===")
    checks = ["architecture", "block_count", "embedding_length", "vocab_size",
              "tokenizer_model", "tokenizer_sha256", "bos_id", "eos_id",
              "padding_id", "chat_template_present", "context_length",
              "head_count", "head_count_kv", "rope_freq_base"]
    for c in checks:
        a, b = out["baseline"].get(c), out["candidate"].get(c)
        same = (a == b)
        print("  %-28s %-8s base=%s cand=%s" % (c, "SAME" if same else "DIFF", a, b))
    json.dump(out, open("/tmp/engine008_meta.json", "w"), indent=1, default=str)
