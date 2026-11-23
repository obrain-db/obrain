# Backend Vulkan (RADV) — stabilité gfx1103 débloquée (2026-09-07, root obtenu)

## Ce qui a été fait
- Root accordé → `apt install libvulkan-dev glslc glslang-tools spirv-headers vulkan-tools`.
- Fork `llama-obrain` (jspace K1/K2 + K4) compilé avec `-DGGML_VULKAN=ON` → `/mnt/obrain/build-vk/bin/llama-server`
  (RADV PHOENIX = gfx1103, Vulkan 1.4.318).
- Le backend Vulkan contourne le ROCm 6.2 cassé (rocBLAS sans TensileLibrary gfx1103 + watchdog) :
  **contexte long + runs répétés STABLES** (plus de crash/HW hang observés sur ces tests).

## Mesures (IQ1_M ~1,9 bpw, 6,27 Go — record config)
| test | tok/s wall | notes |
|---|---|---|
| IQ1_M court n=64 | 6,45 | |
| IQ1_M jspace inject n=96 (contexte long) | **8,74** | état i8 K4 : blob 51,1 MiB ; state_load 633 ms |
| IQ1_M + MTP γ4 n=96 | 7,48 | draft = surcoût ici (pas de gain) |

## Cohérence (question ciblée code sja1000_open, n=96)
**Rappel 4/7** : reset, request_threaded_irq, threaded, **set_reset_mode** (nom exact cité) —
la qualité IQ1_M est réelle sur le pont obrain (première validation longue possible grâce à Vulkan).

## Config retenue
**IQ1_M + Vulkan/RADV, sans MTP, slot np=3** → ~8,7 tok/s stables en contexte réel + K4 états i8.
Suites possibles : tuning MTP côté Vulkan, IQ1_S (~5,8 Go), SIMD du state-load Vulkan, stabilité
très longs runs, multi-questions.

## Compléments (même jour)
- Multi-questions jspace (3 fonctions Linux, IQ1_M, Vulkan) : tok/s 7,0-9,4 ; cohérence ciblée :
  flexcan_close 3/6 (ordre netif_stop_queue→interrupts_disable→free_irq exact), sja 2/7, rkcanfd 0/7
  → IQ1 (~1,9 bpw) garde les rôles mais perd en précision vs Q2/Q3 (compromis vitesse/qualité).
- IQ1_S (5,77 Go) : 8,13 tok/s — même bande qu'IQ1_M (~8-9) : plancher quant atteint sur 780M
  (≈ 50 Go/s ÷ ~6 Go ≈ 8,3 tok/s cohérent).
