# État & plan — « ~1 ms même à froid » (chantiers A & B)

**2026-09-06 · branche feat/zorder · HEAD (chantier B implémenté) · worktree propre.**
Doc de reprise compact : SI contexte perdu, lire ceci + `docs/PROPOSAL-FROID-2026-09-06.md`.

## Contexte (3 lignes)
1. **État réel** : moteur à jour déployé prod+dev (binaire partagé, sha `32a8ade6`,
   backups `obrain.bak-pre-relief-20260905-235905` & `obrain.bak-relief-2dde2245`) ;
   formes de recherche + relief fonctionnelles sur stores réels ; perf CDC : 1,05 Go
   → ~25 Mo / ms-à-chaud (by_ts, lecture fichier, fenêtre glissante, index by_ts) ;
   by_role : chaud 3,3 ms (cache in-process) + froid ~88 ms (sidecar
   `substrate.hks_cache`, 1,78 Mo, parité ok, commit `41cd17f9`).
2. **En cours** : RIEN (chantier B implémenté + chantier A clôturé par constat mesuré).
   Objectif utilisateur : **~1 ms à peu près partout, y compris premier appel d'un
   process neuf** (atteignable en process persistant — déjà le cas ; le plancher
   process-neuf reste l'ouverture).
3. **Prochaine action** : (a) re-bascule binaire prod/dev du chantier B + levier
   open (seuil 8 Mio) avec l'utilisateur (DoD 5, procédure verifier-deploiement,
   poser `OBRAIN_WAL_CHECKPOINT_BYTES` si besoin et vérifier la cadence
   flush/checkpoint du writer) ; (b) le chantier « open ~45 ms » est traité
   (instrumenté + coupable + seuil abaissé) — suiveur optionnel micro-opt du
   scan (gate synthétique).

## Le problème mesuré (process neuf, runtime-dev)
- ouverture substrate **~30-50 ms** (init engine + audit/replay WAL + props) — payée
  par TOUTE requête (même `count(n)`, wall CLI ~80 ms) ;
- chargement fenêtre CDC **~20-40 ms** (1er appel d'une forme, fallback `load()`) ;
- le reste : ms.

## Chantier B — cdc_tail : LIVRÉ (impl + tests + bench, commit unique réversible)
Index de queue `substrate.cdc_tail` (magic CDCT, 20 B/évt : ts/payload_off/frame_len,
anneau 50 k ≈ 1 Mo) tenu à l'append CDC sous le verrou journal (avant `used`) ;
`cold_zone_delta` (cohérence + couverture, jamais autoritaire → fallback) ; voie froide
de `changes_since_timestamp` : décodage des SEULES frames ≥ since + fusion mémoire
session (dédup (entité,epoch,ts), rétention légère symétrique chaud) ; trim au compact.
Bench daté (copie runtime-dev, MÊME copie seedée) : **`delta(60 s)` froid process neuf
121-125 ms → 83-84 ms** (−38 ms) ; `delta(0)` fallback borné inchangé (131 ms) ; RSS
40,2 → 37,8 Mo. Tests : engine défaut 680/680, cdc+algos+ai 732/732 (8 neufs).
**Reste** : re-bascule binaire prod/dev (avec l'utilisateur, procédure verifier-deploiement).
**Piège noté** : l'index n'existe qu'après le 1er append post-déploiement (stores
existants → fallback jusqu'au premier événement ; auto-guérison).

## Chantier A — ouverture froide : REPORTÉ par constat mesuré (recount ≈ 2 ms)
Re-scope MESURÉ au HEAD (copie runtime-dev + prod, CLI release `-v`) : le verify
recount (`rebuild_counts_only`, seul composant que le sidecar `counts_cache`
supprimerait) coûte **~2 ms** (zones nœuds+arêtes ~7 Mo chacune ; « compteurs
vérifiés, déjà justes ms=2 », 2 ms aussi à froid) — l'estimation « recount
10-30 ms » des rows 100-102 n'est pas reproductible. Open chaud count(n) =
83-91 ms : `from_substrate_total` 9 ms, engine `open_substrate` ~49 ms, et
**~45 ms silencieux entre from_substrate_total et le merge WAL-inline** (aucune
trace). ROI sidecar ≈ 2-4 % avec désarmement partiel du filet anti-dérive →
**chantier reporté** (décision honnête type P3c). Vrai levier restant = open
substrate HORS from_substrate (~45 ms) + init engine — chantier
d'instrumentation dédié à ouvrir si « à froid » reste l'objectif.

## Chantier « open substrate ~45 ms » — COUPABLE IDENTIFIÉ + LEVIER APPLIQUÉ
Instrumentation livrée (`707a0df8`) : `from_substrate_core` / `merge_wal_post_flush`
/ total réel. Mesure : store quiescé (WAL 40 o) → open substrate **4 ms**, wall
count(n) 22-23 ms ; WAL 16 Mo → merge **47-49 ms** (~2,9 ms/Mio, garantie zéro
perte), open 60-117 ms. Levier appliqué (`4d343c51`) : seuil d'auto-checkpoint
défaut **512 Mio → 8 Mio** (`OBRAIN_WAL_CHECKPOINT_BYTES`, 0 = désactivé) ⇒ open
borné ≤ ~25 ms de merge en pire cas. Reste au déploiement (bascule) : cadence
flush/checkpoint du writer + option checkpoint périodique 30-60 s (doc engine).
Suiveur optionnel mesuré : micro-opt du scan (ids en payload, pas en en-tête —
gate synthétique requise avant refonte).

## Pièges appris (à ne pas refaire)
- **Disque plein** (100 %) : purge `target/debug` (~52 G) + copies /tmp — faux
  échecs de tests = ENOSPC (rustc ICE, link cc, 823/47). Vérifier `df -h` avant gros build.
- **Binaire en cours d'exécution** : swap par **mv** (rename), pas cp (ETXTBSY).
- **count d'en-tête CDC périmé** après compaction → fenêtre glissante d'offsets, PAS
  keep_from par count (commit `4a947591`) ; idem pour l'index cdc_tail : cohérence
  vérifiée contre `used` à chaque lecture, jamais de count.
- **`seek_relative` + BufReader** : la passe 1 de `cdc_persist::load()` s'arrête
  ~19,9 Mo / 174 k frames sur la copie runtime (réplique) alors que la marche à seek
  direct atteint `used` (1,66 M frames) — OBSERVATION pré-existante à investiguer
  (chantier dédié : seek absolu) ; cdc_tail n'emprunte pas ce chemin pour le delta récent.
- Tests : engine 680/680 (défaut) · cdc+algos+ai 732/732 · formes search_* vertes.

## Portes & artefacts
- Propositions : `docs/PROPOSAL-FROID-2026-09-06.md`, `docs/PROPOSAL-OPTIM-BY-ROLE-2026-09-06.md`.
- Ledger : rangées 09-05/06 (perf CDC, bascule, tests bout-en-bout, caches, cdc_tail).
- Vérif rapide réelle : bridge RO sur runtime-dev, `CALL obrain.relief/delta/by_role`.
