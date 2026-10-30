# Porte post-déploiement — continuité d'écriture (C4, 2026-09-05)

**Objet** : après CHAQUE bascule binaire (prod ou dev), vérifier que le
writer obrain est vivant et que la base n'est pas figée — rejouable, chiffrée.

## Commande rejouable

```bash
# writer(s) obrain actifs (bridge OBRAIN_WRITE) :
pgrep -f "obrain bridge" | wc -l

# fraîcheur de la base (seuil par env — prod tolérant à l'inactivité) :
obrain continuity ~/.local/share/obrain-harness/data/runtime-dev.obrain  --stale-after-min 5   # dev strict
obrain continuity ~/.local/share/obrain-harness/data/runtime-prod.obrain --stale-after-min 60  # prod
# exit 0 = frais ; exit 1 = STALE (aucune écriture de journal depuis N min)
```

Intégré au vérificateur de déploiement existant : `scripts/verifier-deploiement.sh`
— nouvelle **Porte 5 « continuité »** (writer ≥ 1 actif + `continuity` exit 0 ;
seuil par env, surchargeable `SEUIL_CONT`).

## Attendu après une bascule saine

- `pgrep -f "obrain bridge"` ≥ 1 (le writer a été respawné par le service) ;
- `obrain continuity` : exit 0, `journals` > 0, `last_event_age_sec` < seuil
  quand une session est active (dev) — prod inactif toléré (seuil 60 min) ;
- en cas de STALE pendant une activité attendue : investiguer immédiatement
  (writer mort/absent → respawn manuel ou redémarrage du service, puis le
  C2 re-push JSONL→graphe rattrape le log de façon idempotente).

## Chiffres de référence (2026-09-05)

- dev (runtime-dev, session active) : `last_event_age_sec` ~ 20 s — frais ;
- prod (runtime-prod) au 05/09 21:2x : STALE ~21,5 min (inactif ou réel — à
  confirmer selon l'activité attendue de dsh-web) : la porte fait son travail.
