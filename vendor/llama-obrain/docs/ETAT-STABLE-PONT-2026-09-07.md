# PONT obrain↔llama — ÉTAT STABLE (2026-09-07, machine n3)

> **Depuis (2026-09-20) — la ternarisation est entrée dans ce fork.** Type ggml `TQ2_G128`
> (groupe 128, cases de 2 bits, 2,125 bpw), op `GGML_OP_WHT` de premier ordre, crochet dans
> `build_lora_mm`, repli hors ligne dans le quantifieur, et **politique de tenseurs pilotable par
> variables d'environnement** (10 minutes par essai). Candidat :
> `Qwen3.8-27B-OBRAIN-v2.gguf` — **11,61 Go**, perplexité **4,74**, **raisonne** (contre 18,97 Go et
> 2,40 pour `Q4_K_M`) ; meilleur point connu non sauvegardé : **4,29 à 12,17 Go**.
>
> **À savoir pour ce pont** : le modèle ternaire **reste sur le CPU** — mesuré 2,01 tok/s sur le
> binaire Vulkan contre 1,90 en CPU pur — tant que `TQ2_G128` n'a pas de noyau `mul_mat_vec` Vulkan
> et `GGML_OP_WHT` de shader. Le MTP est un **levier GPU** (−24 % sur CPU, mesuré).
>
> **Reprise** : `docs/REPRISE-TERNARISATION-2026-09-20.md` · **récit technique** :
> `docs/TERNARISATION-2026-09-20.md`.

## Configuration consolidée (validée, reproductible)
- **Backend** : Vulkan/RADV (`/mnt/obrain/build-vk/bin/llama-server`) — stable sur gfx1103
  (contourne ROCm 6.2 : rocBLAS gfx1103 absent + watchdog).
- **Modèle** : Qwen3.8-27B `UD-Q2_K_XL` (9,15 Go, qualité) — variante vitesse : `UD-IQ1_M` (6,27 Go).
- **États K4** : `OBRAIN_STATE_TYPE=i8` (blob ÷3, cohérence vérifiée) ; cache disque `kv-states/` à vider si le binaire change (magic bumpé).
- **Store** : `/tmp/kv-linux-driversnet.obrain` (git Linux 7.3-rc2, 117 043 fonctions).

## Utilisation
```bash
# 1) serveur (3 slots jspace) :
bash ops-gpu16/run_pont_vk.sh                     # Q2_K_XL (qualité) — port 8099
#    variante vitesse : bash ops-gpu16/run_pont_vk.sh /mnt/obrain/models/Qwen3.8-27B-UD-IQ1_M.gguf
# 2) question→réponse branché sur obrain (sélection des fonctions par mots-clés dans le store) :
python3 ops-gpu16/qa_obrain.py "Que fait la fonction sja1000_open ?"
```

## Mesures (Q2_K_XL, Vulkan, état i8)
- extract (fait ~200-225 tok) : ~4,3 s ; blob **50-51 MiB** ; state_load ~0,65-0,68 s ;
- décodage : **5,5-7,7 tok/s** (selon longueur), TFPT serveur ~5-18 ms ;
- cohérence : réponses structurées (think puis réponse) fondées sur le code réellement injecté.

## Bilan campagne (clôtures documentées)
- Mesures : murs BW (~50 Go/s), spectres plats, Δ plein-rang, fenêtres non représentatives → les
  familles Δ, bas-rang, fenêtre (FTC/IRT/SFM) et WDT-GDN (f≈11 %) sont closes pour 50 tok/s mono.
- Config stable ≈ **8-9 tok/s** (IQ1_M) / **6-8 tok/s** (Q2_K_XL) — plafond matériel du 27B sur la 780M.
