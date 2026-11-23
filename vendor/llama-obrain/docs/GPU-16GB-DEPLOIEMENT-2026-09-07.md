# Déploiement GPU 16 Go — Qwen3.8-27B ≥ 50 tok/s (kit 2026-09-07)

Décisions prises (utilisateur : « à toi de voir ») : modèle **Qwen3.8-27B-UD-Q3_K_XL** (unsloth,
13,1 Go — seul bon compromis qui **tient en entier dans 16 Go de VRAM**) + **draft MTP** pour le
décodage spéculatif. Engine = `llama-obrain-dgx` (merge upstream 2026-07 : MTP/Eagle3, flash-attn,
jspace `/jspace/inject_knowledge` conservé — vérifié `jspace_reserved_slot_id` présent).

## 1. Fichiers modèles (déjà téléchargés, machine n3 → `/mnt/obrain/models/`)

| fichier | taille | rôle |
|---|---|---|
| `Qwen3.8-27B-UD-Q3_K_XL.gguf` | 13 146 393 504 o (12,24 GiB) | modèle principal (qwen35, ctx 262144, hybride attention+SSM) |
| `mtp-Qwen3.8-27B-Q4_0.gguf` | 1 369 590 656 o (1,28 GiB) | draft MTP (décodage spéculatif) |

Récupérer sur la machine GPU : `scp -r <n3>:/mnt/obrain/models/ /mnt/obrain/models/` (ou copie
manuelle). Alternative si Q3 jugé trop agressif : `Qwen3.8-27B-UD-IQ4_XS.gguf` (14,3 Go — frôle la
limite avec KV) ; si la carte est faible en bande passante : `Qwen3.8-27B-UD-Q2_K_XL.gguf` (9,8 Go).

## 2. Engine (machine GPU, CUDA toolkit ≥ 12.4)

```bash
# depuis le checkout engine sur la machine GPU (repo llama-obrain-dgx)
bash <n3>/home/triviere/projects/llama-obrain/ops-gpu16/build_cuda.sh /chemin/llama-obrain-dgx 89
# arch : 89 = RTX 40xx (Ada), 90 = Blackwell, 86 = Ampere, 'native' = auto
```

Flags build : `-DGGML_CUDA=ON -DGGML_CUDA_FA=ON -DCMAKE_CUDA_ARCHITECTURES=89 -DCMAKE_BUILD_TYPE=Release`
(FA = kernels FlashAttention CUDA compilés ; l'option « fast dequant all quants » n'existe plus sous
ce nom dans cette version ggml — inutile).

## 3. Lancement (injection KV / jspace opérationnel)

```bash
USE_MTP=1 bash run_server_gpu.sh /mnt/obrain/models/Qwen3.8-27B-UD-Q3_K_XL.gguf 8099 8192
# ou USE_MTP=0 pour comparer sans spéculatif ; KV q8_0 par défaut (÷2 VRAM)
```

Vérifs : `/health` ok, `nvidia-smi` → VRAM utilisée ~13-14,5 Go (pas d'OOM), `curl /slots` →
`n_ctx_slot` ≈ ctx/3.

## 4. Bench (tok/s + TFPT, à faire sur la machine GPU — non testable sur n3 sans GPU)

```bash
# a) brut (llama-bench, tg = décodage) :
llama-bench -m .../Q3_K_XL.gguf -ngl 99 -p 64 -n 128
# b) boucle réelle injection KV (obrain-adapter, après copie du repo) :
python3 obrain-adapter/bench_ctx2500.py <obrain-bin> <store-linux.obrain> 8099 128
```

## 5. Attendu (théorie bande passante, tok/s ≈ BW ÷ poids ; réel ≈ 75-90 %)

| GPU 16 Go | BW | Q3_K_XL (12,24 GiB) | avec MTP (~+20-40 %) |
|---|---|---|---|
| RTX 4080 SUPER | 736 Go/s | **~55-60** | ~70+ |
| RTX 4080 | 717 Go/s | **~52-58** | ~65+ |
| RTX 4070 Ti SUPER | 672 Go/s | **~48-54** | ~60 |
| RTX 4060 Ti 16 Go | 288 Go/s | ~19-22 | ~26 (27B non adapté) |

→ **≥ 50 tok/s : atteignable sur 4080(S)/4070TiS avec Q3_K_XL (full offload), le MTP donne la marge.**
Sur carte faible (4060 Ti) : 27B plafonné ; basculer sur Qwen3.5-9B (5,6 Go) pour la boucle, garder
le 27B pour la DGX. Le contexte 256k natif du modèle n'est PAS utilisable en 16 Go (KV pleine
fenêtre ≈ 17 Go) — `-c 8192` pour le pont, 256k réservé à la DGX.

## 6. Fichiers du kit

- `build_cuda.sh` — build CUDA engine (arch paramétrable)
- `run_server_gpu.sh` — lanceur serveur (jspace, KV q8, MTP optionnel)
- Docs associées : `docs/COMPRESSION-X2500-2026-09-07.md`, `docs/PERF-CONTEXTE-2500-2026-09-07.md`
