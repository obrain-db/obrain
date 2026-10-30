#!/usr/bin/env python3
"""ENGINE-008 HARNESS -- autorites de METADONNEES, sans aucun decode.

But : etablir, SANS libllama et SANS llama_decode, les autorites qui vivent
entierement dans l en-tete GGUF :

  - SHA256 du fichier modele (recontrole de l autorite §13)
  - architecture, nombre de couches normales, couches NextN (§53)
  - resume qtype (par type ggml, poids total des tenseurs)
  - SHA256 du tokenizer au sens de ENGINE-008 (tokens concatenes, NUL-separes)
    -> recontrole de TOKENIZER_SHA256 (§14)
  - VOCAB_SIZE / BOS / EOS / PADDING (§14)
  - CHAINE EXACTE du chat template + SHA256 (§15)
  - predicat BOS du template (§19) : le template emet-il un token BOS ?

AUCUNE inference. Aucun logit. Aucun llama_decode. Lecture d en-tete seule.
Le hachage du tokenizer est l EXACTE convention de engine008-gguf-meta.py :
pour chaque token, sha.update(token.encode(utf-8, replace)) puis sha.update(b"\\x00").
"""
import hashlib
import json
import os
import struct
import sys

# ---- types ggml (ggml/include/ggml.h, arbre qualitative Q1) ----------------
GGML_TYPES = {
    0: "F32", 1: "F16", 2: "Q4_0", 3: "Q4_1", 6: "Q5_0", 7: "Q5_1",
    8: "Q8_0", 9: "Q8_1", 10: "Q2_K", 11: "Q3_K", 12: "Q4_K", 13: "Q5_K",
    14: "Q6_K", 15: "Q8_K", 16: "IQ2_XXS", 17: "IQ2_XS", 18: "IQ3_XXS",
    19: "IQ1_S", 20: "IQ4_NL", 21: "IQ3_S", 22: "IQ2_S", 23: "IQ4_XS",
    24: "I8", 25: "I16", 26: "I32", 27: "I64", 28: "F64", 29: "IQ1_M",
    30: "BF16", 34: "TQ1_0", 35: "TQ2_0", 39: "MXFP4", 40: "NVFP4",
    41: "Q1_0", 42: "Q2_0", 43: "TQ2_G128",
}

# taille en octets du bloc de chaque type, et nombre de poids par bloc
# (uniquement pour le resume qtype ; valeurs du catalogue ggml)
BLOCK = {
    0: (1, 1), 1: (2, 1), 2: (18, 32), 3: (20, 32), 6: (22, 32), 7: (24, 32),
    8: (34, 32), 9: (36, 32), 10: (84, 256), 11: (110, 256), 12: (144, 256),
    13: (176, 256), 14: (210, 256), 15: (292, 256), 16: (66, 256),
    17: (74, 256), 18: (98, 256), 19: (50, 256), 20: (18, 32), 21: (110, 256),
    22: (82, 256), 23: (34, 32), 24: (1, 1), 25: (2, 1), 26: (4, 1),
    27: (8, 1), 28: (8, 1), 29: (56, 256), 30: (2, 1), 34: (34, 256),
    35: (34, 256), 39: (17, 32), 40: (36, 64), 43: (34, 128),
}

T_U8, T_I8, T_U16, T_I16, T_U32, T_I32, T_F32, T_BOOL, T_STR, T_ARR, \
    T_U64, T_I64, T_F64 = range(13)

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
        if et == T_STR:
            return [read_str(f) for _ in range(n)]
        if et == T_ARR:
            return [read_val(f, T_ARR) for _ in range(n)]
        fmt, sz = SCAL[et]
        return list(struct.unpack("<%d%s" % (n, fmt[1]), f.read(sz * n)))
    raise ValueError("type GGUF %d inconnu" % t)


def load_gguf(path):
    """Lit l en-tete COMPLET (KV sans troncature) + les infos de tenseurs."""
    meta, tensors = {}, []
    with open(path, "rb") as f:
        magic = f.read(4)
        if magic != b"GGUF":
            raise ValueError("magic=%r" % magic)
        version = struct.unpack("<I", f.read(4))[0]
        n_tensors = struct.unpack("<Q", f.read(8))[0]
        n_kv = struct.unpack("<Q", f.read(8))[0]
        for _ in range(n_kv):
            k = read_str(f)
            t = struct.unpack("<I", f.read(4))[0]
            meta[k] = read_val(f, t)
        for _ in range(n_tensors):
            name = read_str(f)
            nd = struct.unpack("<I", f.read(4))[0]
            dims = list(struct.unpack("<%dQ" % nd, f.read(8 * nd)))
            tt = struct.unpack("<I", f.read(4))[0]
            off = struct.unpack("<Q", f.read(8))[0]
            tensors.append({"name": name, "dims": dims, "type": tt, "off": off})
    return version, meta, tensors


def sha256_file(path, chunk=1 << 22):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for b in iter(lambda: f.read(chunk), b""):
            h.update(b)
    return h.hexdigest()


def tokenizer_sha(tokens):
    """Convention EXACTE de engine008-gguf-meta.py."""
    h = hashlib.sha256()
    for t in tokens:
        h.update(t.encode("utf-8", "replace"))
        h.update(b"\x00")
    return h.hexdigest()


def qtype_summary(tensors):
    per = {}
    for t in tensors:
        n_elem = 1
        for d in t["dims"]:
            n_elem *= d
        per.setdefault(t["type"], {"tensors": 0, "elements": 0})
        per[t["type"]]["tensors"] += 1
        per[t["type"]]["elements"] += n_elem
    rows = []
    for tt in sorted(per):
        bs, be = BLOCK.get(tt, (0, 1))
        n_elem = per[tt]["elements"]
        nbytes = (n_elem // be) * bs if be else 0
        rows.append({"type_id": tt, "type_name": GGML_TYPES.get(tt, "UNKNOWN"),
                     "tensors": per[tt]["tensors"], "elements": n_elem,
                     "bytes_est": nbytes})
    return rows


def layer_inventory(tensors):
    """Couches normales blk.N, couches NextN (blk.N.nextn.* / eh_proj / etc.)."""
    normals, nextn, other = set(), set(), set()
    for t in tensors:
        n = t["name"]
        if n.startswith("blk."):
            parts = n.split(".")
            try:
                idx = int(parts[1])
            except (IndexError, ValueError):
                other.add(n)
                continue
            if len(parts) > 3 and parts[2] == "nextn":
                nextn.add(idx)
            else:
                normals.add(idx)
        else:
            other.add(n)
    return sorted(normals), sorted(nextn), sorted(other)


def emits_bos(tmpl):
    """Predicat STATIQUE seulement : la chaine contient-elle une emission BOS ?
    On ne devine pas le rendu ; on cherche les marqueurs de controle connus."""
    if tmpl is None:
        return None
    markers = ["<|startoftext|>", "<s>", "{{ bos_token }}", "{{- bos_token }}"]
    return any(m in tmpl for m in markers)


def audit(label, path):
    print("=" * 72)
    print("=== %s ===" % label.upper())
    print("PATH=%s" % path)
    file_sha = sha256_file(path)
    print("MODEL_SHA256=%s" % file_sha)
    version, meta, tensors = load_gguf(path)

    arch = meta.get("general.architecture")
    def g(*keys):
        for k in keys:
            if k in meta:
                return meta[k]
        return None

    ctx = g("%s.context_length" % arch, "qwen35.context_length",
            "qwen3.context_length")
    n_layer = g("%s.block_count" % arch, "qwen35.block_count",
                "qwen3.block_count")
    vocab = g("%s.vocab_size" % arch, "qwen35.vocab_size", "qwen3.vocab_size")
    n_nextn = g("%s.nextn_predict_layers" % arch,
                "qwen35.nextn_predict_layers", "qwen3.nextn_predict_layers")

    tokens = meta.get("tokenizer.ggml.tokens")
    tok_sha = tokenizer_sha(tokens) if isinstance(tokens, list) else None
    tmpl = meta.get("tokenizer.chat_template")
    tmpl_sha = (hashlib.sha256(tmpl.encode("utf-8")).hexdigest()
                if isinstance(tmpl, str) else None)

    normals, nextn, other = layer_inventory(tensors)

    print("GGUF_VERSION=%s" % version)
    print("ARCHITECTURE=%s" % arch)
    print("TENSOR_COUNT=%d" % len(tensors))
    print("KV_COUNT=%d" % len(meta))
    print("CONTEXT_LENGTH=%s" % ctx)
    print("BLOCK_COUNT_HEADER=%s" % n_layer)
    print("NEXTN_PREDICT_LAYERS_HEADER=%s" % n_nextn)
    print("NORMAL_DECODER_LAYER_INDICES=%d" % len(normals))
    print("NORMAL_DECODER_LAYER_MINMAX=%s..%s" %
          (normals[0] if normals else None, normals[-1] if normals else None))
    print("NEXTN_LAYER_INDICES=%s" % (nextn if nextn else "[]"))
    print("NEXTN_TENSOR_COUNT=%d" %
          sum(1 for t in tensors if ".nextn." in t["name"]))
    print("VOCAB_SIZE_HEADER=%s" % vocab)
    print("TOKENIZER_MODEL=%s" % meta.get("tokenizer.ggml.model"))
    print("TOKENIZER_PRE=%s" % meta.get("tokenizer.ggml.pre"))
    print("TOKENIZER_TOKEN_COUNT=%s" % (len(tokens) if isinstance(tokens, list) else None))
    print("TOKENIZER_SHA256=%s" % tok_sha)
    print("BOS_TOKEN_ID=%s" % meta.get("tokenizer.ggml.bos_token_id"))
    print("EOS_TOKEN_ID=%s" % meta.get("tokenizer.ggml.eos_token_id"))
    print("PADDING_TOKEN_ID=%s" % meta.get("tokenizer.ggml.padding_token_id"))
    print("ADD_BOS_TOKEN=%s" % meta.get("tokenizer.ggml.add_bos_token"))
    print("ADD_EOS_TOKEN=%s" % meta.get("tokenizer.ggml.add_eos_token"))
    print("CHAT_TEMPLATE_PRESENT=%s" % ("YES" if isinstance(tmpl, str) else "NO"))
    print("CHAT_TEMPLATE_SHA256=%s" % tmpl_sha)
    print("CHAT_TEMPLATE_LEN_BYTES=%s" %
          (len(tmpl.encode("utf-8")) if isinstance(tmpl, str) else None))
    print("CHAT_TEMPLATE_EMITS_BOS_MARKER=%s" % emits_bos(tmpl))
    print("QTYPE_SUMMARY:")
    for r in qtype_summary(tensors):
        print("  type=%-9s id=%-3d tensors=%-5d elements=%-14d bytes~%d" %
              (r["type_name"], r["type_id"], r["tensors"], r["elements"],
               r["bytes_est"]))
    print("CHAT_TEMPLATE_BEGIN>>>")
    if isinstance(tmpl, str):
        print(tmpl)
    print("<<<CHAT_TEMPLATE_END")
    print()

    return {
        "label": label, "path": path, "model_sha256": file_sha,
        "gguf_version": version, "architecture": arch,
        "tensor_count": len(tensors), "kv_count": len(meta),
        "context_length": ctx, "block_count_header": n_layer,
        "nextn_predict_layers_header": n_nextn,
        "normal_layer_indices": normals,
        "nextn_layer_indices": nextn,
        "nextn_tensor_count": sum(1 for t in tensors
                                  if ".nextn." in t["name"]),
        "all_tensor_names": [t["name"] for t in tensors],
        "other_tensor_names": sorted(other),
        "vocab_size_header": vocab,
        "tokenizer_model": meta.get("tokenizer.ggml.model"),
        "tokenizer_pre": meta.get("tokenizer.ggml.pre"),
        "tokenizer_token_count": len(tokens) if isinstance(tokens, list) else None,
        "tokenizer_sha256": tok_sha,
        "bos_token_id": meta.get("tokenizer.ggml.bos_token_id"),
        "eos_token_id": meta.get("tokenizer.ggml.eos_token_id"),
        "padding_token_id": meta.get("tokenizer.ggml.padding_token_id"),
        "add_bos_token": meta.get("tokenizer.ggml.add_bos_token"),
        "add_eos_token": meta.get("tokenizer.ggml.add_eos_token"),
        "chat_template": tmpl,
        "chat_template_sha256": tmpl_sha,
        "chat_template_emits_bos_marker": emits_bos(tmpl),
        "qtype_summary": qtype_summary(tensors),
    }


if __name__ == "__main__":
    base, cand = sys.argv[1], sys.argv[2]
    outbase = sys.argv[3] if len(sys.argv) > 3 else "/tmp/engine008_authority"
    a = audit("baseline", base)
    b = audit("candidate", cand)

    print("=" * 72)
    print("=== IDENTITE LOGIQUE (baseline vs candidat) ===")
    same_keys = ["architecture", "context_length", "vocab_size_header",
                 "tokenizer_model", "tokenizer_pre", "tokenizer_token_count",
                 "tokenizer_sha256", "bos_token_id", "eos_token_id",
                 "add_bos_token", "add_eos_token", "chat_template_sha256",
                 "block_count_header"]
    verdict = {}
    for k in same_keys:
        s = (a[k] == b[k])
        verdict[k] = "SAME" if s else "DIFF"
        print("  %-28s %-6s base=%s cand=%s" % (k, verdict[k], a[k], b[k]))

    print()
    print("=== DIFFERENCES ATTENDUES / INTERDITES ===")
    for k in ["padding_token_id", "nextn_predict_layers_header"]:
        s = (a[k] == b[k])
        print("  %-28s %-6s base=%s cand=%s" %
              (k, "SAME" if s else "DIFF", a[k], b[k]))
    print("  normal_layer_count           base=%d cand=%d" %
          (len(a["normal_layer_indices"]), len(b["normal_layer_indices"])))
    print("  nextn_layer_indices          base=%s cand=%s" %
          (a["nextn_layer_indices"], b["nextn_layer_indices"]))

    qa = {r["type_name"] for r in a["qtype_summary"]}
    qb = {r["type_name"] for r in b["qtype_summary"]}
    print("  qtypes_baseline=%s" % sorted(qa))
    print("  qtypes_candidate=%s" % sorted(qb))
    print("  qtypes_only_candidate=%s" % sorted(qb - qa))

    json.dump({"baseline": a, "candidate": b, "identity": verdict},
              open(outbase + ".json", "w"), indent=1)
    print()
    print("JSON=%s.json" % outbase)
