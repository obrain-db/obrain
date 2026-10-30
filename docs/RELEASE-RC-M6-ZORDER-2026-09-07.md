# Release candidate — montée dev/prod : feat/release-m6-zorder

**Date** : 2026-09-07 · branche `feat/release-m6-zorder` · merge `50f10d9a`
= `feat/zorder` (slots 512 + fix remap P0 + QC/flip) **+** `feat/m6-code-graph`
(indexer natif optimisé : queries µs, indexation mono-passage, obrain-indexer).

## Suites (RC, 0 rouge)
- substrate lib **822/822 + 1 ignoré** · engine **683/683** · invariants **9/9**
- obrain-indexer : **80 + 37** tests · build CLI + exemples QC OK

## Pré-vol sur copies réelles (binaire RC, env slots=1)
| Contrôle | runtime-dev | runtime-prod |
|---|---|---|
| QC parité (counts + écritures contrôlées) | ✅ | ✅ |
| Migration multi-phase (A,U,C,D,E×3, comptes exacts) | ✅ | ✅ |
| Cognitif (digest inchangé, zones identiques) | ✅ | — (dev représentatif) |

## Binaire de montée
`target/release/obrain` — sha256 `c0d30668…71369` (RC).
Lot de déploiement prêt : `~/slots512-deploy/` (DEPLOY-SLOTS512.md,
MANIFEST-slots512-TEMPLATE.txt, drop-in env dev).
Process : dev d'abord (dsh-dev-3083), prod ensuite (dsh-web + snapshot base) —
voir DEPLOY-SLOTS512.md. Bump SUBSTRATE_FORMAT_VERSION 1→2 : phasé (non requis
tant que l'env slots reste OFF en prod).
