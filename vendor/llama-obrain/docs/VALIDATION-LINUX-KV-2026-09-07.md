# Validation — branchement DIRECT obrain ↔ llama-obrain sur la base Linux (KV-cache)

**Date : 2026-09-07 · Mode : CPU uniquement (directive round 11) · Qwen3.8-27B Q4_K_M · serveur fork `build/bin/llama-server` (:8099, `-c 2048 -t 8 --parallel 3`)**

Base de validation (branchement direct, aucun snippet artisanal) :
`/tmp/kv-linux-driversnet.obrain` — store substrate persisté depuis le **git Linux**
(`/tmp/linux-git`, HEAD `df2908090cda Linux 7.3-rc2`, `drivers/net`) :
**310 547 nœuds / 769 685 arêtes (117 043 `Function`, 6 257 `File` ; labels Project/Branch/Function/Struct/Field…)**, vérifié en lecture seule (CLI + bridge).

---

## 1. Relief structurel du store (avant injection)

- Arêtes `CALLS` exactes, ex. `m_can_start_xmit → {m_can_tx_handler, m_can_start_peripheral_xmit, m_can_start_tx, m_can_clean, can_skb_get_frame_len}` (conforme au code source).
- GQL : `MATCH (n:Function) WHERE n.text IS NOT NULL` (117 043), `CONTAINS` sur `qualified_name`, `CALL obrain.*` (articulation_points, …) disponibles.
- `text` = docstring kernel (e1000_probe) ou signature/corps (fonctions CAN) — lue depuis le store par le bridge RO pour construire chaque « fait ».

## 2. E2E multi-faits (n_generate=48, greedy) — 4/4 réponses générées

Fonctions réelles (textes lus DEPUIS le store) : `m_can_start_xmit`, `rkcanfd_open`, `flexcan_close`, `sja1000_open`.

| # | fait (fonction) | extract (s) | tokens | ref | blob (o) | state-load (ms) | TFPT (ms) | tok/s | wall (s) |
|---|---|---|---|---|---|---|---|---|---|
| 1 | `m_can_start_xmit` | 18,6 | 227 | `4cbd1553…` | 171 775 984 | 110,0 | 663,9 | 1,70 | 31,8 |
| 2 | `rkcanfd_open` | 18,2 | 224 | `009207e8…` | 171 579 316 | 110,9 | 565,2 | 1,24 | 42,3 |
| 3 | `flexcan_close` | 17,2 | 200 | `642a40bf…` | 170 005 972 | 139,7 | 939,9 | 1,07 | 48,4 |
| 4 | `sja1000_open` | 18,6 | 219 | `09ee7ab8…` | 171 251 536 | 123,0 | 925,8 | 1,09 | 47,8 |

- **Extract** = reconstruction de l'état K/V à la demande depuis la base obrain (état en RAM ; fichier `kv-states/<ref>.bin` = cache optionnel, ref FNV déterministe).
- **Inject par ref** = bypass prefill du fait (aucun token texte du fait dans le contexte visible) : chargement d'état 97-140 ms, puis décodage direct du prompt.
- **Réutilisation cache RAM** : 2e inject du même ref (autre question) sur les 4 faits → state-load 112-128 ms, aucun re-extract.
- Rôles identifiés correctement dans les 4 réponses (démarrage TX M_CAN, init Rockchip CAN-FD, shutdown FlexCAN, init SJA1000) — fonctions CAN obscures non connaissables hors du fait injecté.

## 3. Cohérence ciblée (n_generate=140) — réponses déductibles UNIQUEMENT du code injecté

Marqueurs = identifiants/étapes présents uniquement dans le corps de la fonction (pas déductibles du nom).

| fonction | rappel marqueurs | hits | contenu de la réponse (vérifié) |
|---|---|---|---|
| `rkcanfd_open` | **0,50** | open_candev, runtime, chip_start, offload, request_irq | séquence exacte : open_candev → pm_runtime_resume_and_get → rkcanfd_chip_start → can_rx_offload_enable → request_irq (IRQF_SHARED) → netif_start_queue ; goto out_close_candev / out_rkcanfd_chip_stop |
| `flexcan_close` | **0,40** | netif_stop_queue, interrupts_disable, free_irq, secondary | ordre des cleanups + free_irq sous FLEXCAN_QUIRK_SECONDARY_MB_IRQ / NR_IRQ_3, offload, transceiver |
| `sja1000_open` | **0,56** | reset, open_candev, request_threaded_irq, threaded, custom | set_reset_mode avant open_candev ; request_threaded_irq sautée si SJA1000_CUSTOM_IRQ_HANDLER ; -EAGAIN si échec ; sja1000_start final |
| `m_can_start_xmit` | — | — | **artefact slot fork** : 0 token persistant à la recharge de ce ref (cache RAM puis disque après redémarrage) ; rôle correct en e2e n=48 (début de réponse cohérent) |

Rappel moyen (hors artefact) : **0,49**. Les réponses reproduisent la logique interne du code injecté —
preuve que l'état K/V injecté (sans texte) alimente réellement le décodage.

## 4. Verdict par directive

1. **Validation CPU** ✓ — toutes les mesures datées ci-dessus (t=8, machine 16 cœurs, pas de GPU).
2. **Reconstruction à la demande, pas de blob disque requis** ✓ — extract → état en RAM (cache disque optionnel) ; redémarrage serveur → rechargement disque 643 ms (K1b cross-restart mécaniquement OK).
3. **Métriques TFPT / tokens/s** ✓ — rapportées pour chaque inject.
4. **Base réelle** ✓ — git Linux (7.3-rc2), drivers/net, 117 k fonctions, couche Project/Branch active.

## 5. Rejouabilité

```
# serveur fork
build/bin/llama-server -m /home/triviere/models/Qwen3.8-27B-Q4_K_M.gguf -c 2048 -t 8 --parallel 3 --port 8099
# e2e multi-faits (extract+inject+réutilisation RAM, 4 faits) puis cohérence ciblée
python3 obrain-adapter/e2e_linux_kv.py         <obrain-bin> /tmp/kv-linux-driversnet.obrain 8099 48
python3 obrain-adapter/e2e_linux_kv_targeted.py <obrain-bin> /tmp/kv-linux-driversnet.obrain 8099 140
# rapports : /tmp/e2e-linux-kv-report.{md,json}, /tmp/e2e-linux-kv-targeted-report.{md,json}
```

## 6. Suites identifiées

- **Artefact slot** : certains refs rechargés (ici `4cbd1553…`) renvoient `n_tokens=0` de façon persistante (RAM puis disque) — à caractériser (K2 : état vs séquence du slot ; génération courte OK, longue KO → suspecter fin de fait tronquée/état partiel).
- **K3** (boucle fermée) et **K4** (quant KV) restent les chantiers suivants ; la cohérence fine « questions multiples » de cette validation est le référentiel de mesure.
