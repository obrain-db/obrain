# CONFINEMENT DES CHEMINS STATIQUES — la garde portée dans la lignée servie

*2026-09-15. Ferme la lecture de fichier arbitraire mesurée sur le binaire du
8432. Dit aussi pourquoi le contrôle qui « vérifiait » cette faille a menti
pendant quelques heures.*

## 1. La faille, mesurée

`GET /app/…` joignait le chemin demandé à la racine servie **sans décodage ni
assainissement** :

```
GET /app/../../../../../../../../etc/hostname   → 200 + le contenu
GET /app/…/etc/passwd                           → 200 + le fichier
```

Tout ce que le processus peut lire, y compris hors de la garde IAM (elle ne
couvre que `/api/**`), sur un service exposé en LAN (`192.168.1.103:8432`).

## 2. Pourquoi le contrôle a dit PASS à tort

`scripts/protocole-check.mjs` envoyait **sept** `../` — valeur calibrée sur
l'ancienne racine (`…/banc-obrain-j5/packages/obrain-dashboard/dist`). Quand ce
`dist` est devenu un **lien** vers le magasin de versions
(`…/.local/share/obrain-harness/ui/current`, chemin **plus profond**), sept
niveaux ont cessé d'atteindre `/etc` : le contrôle est passé à PASS **alors que
la fuite était intacte, simplement décalée**. Re-mesuré :

| profondeur | avant correctif |
|---|---|
| 7 | 404 |
| 9 · 11 · 13 · 15 · 18 · 20 | **200 + contenu** |

Le contrôle teste désormais la **capacité d'échappement** de 7 à 20 niveaux, et
échoue si **une seule** réussit. Une profondeur fixe ne prouve rien : la racine
servie peut bouger.

## 3. Le correctif

Porté de la lignée du tronc, qui le portait déjà (`viz_serve.rs`, fonction
`chemin_sous`) :

1. **décodage pourcent du chemin** (`percent_decode_chemin` — variante de
   `percent_decode`, où `+` est un caractère ordinaire et non un espace) ;
2. refus du composant `..` ou `.`, du `/` initial, du `\` et du NUL —
   **composant par composant**, et non « contient `..` » : un fichier nommé
   `index..js` est légitime ;
3. après `canonicalize`, le chemin réel doit rester **sous** la racine — c'est ce
   verrou qui arrête l'échappement par **lien symbolique**, où aucun `..`
   n'apparaît jamais.

Deux verrous, qui ne font pas double emploi : le premier arrête l'attaque par le
nom, le second celle par le lien.

Réponse en cas de refus : **404**, jamais 403 — dire « interdit » confirmerait
l'existence d'un fichier hors du dossier.

## 4. Vérifié

Sur instance isolée **puis sur le service vivant** (binaire `e169eb0c`, service
redémarré à 11:2x, sauvegarde `~/sauvegardes-bascule-v3/obrain.avant-garde-confinement-20260915`) :

| Contrôle | Résultat |
|---|---|
| traversée 7 · 9 · 11 · 13 · 15 · 18 · 20 niveaux | **404 partout** |
| traversée encodée (`%2e%2e`) | 404 |
| traversée mixte (`…/dist/…`, `./…`) | 404 |
| servir l'UI (`/`, bundle 1 327 603 o, CSS 400 399 o) | **200** |
| `/sitemap.xml` | 200 |
| `ui-verifier.sh` | **`UI SERVIE CONFORME`** |
| `overview` à froid (opérations nommées) | 0,508 ms |
| transport WS | frames `begin nodes edges end`, 15 nœuds / 14 arêtes |
| contrôle de protocole | traversée **PASS (7/7 refusées)** — 10 échecs restants, tous préexistants |

**Retour arrière** :

```bash
install -m 755 ~/sauvegardes-bascule-v3/obrain.avant-garde-confinement-20260915 \
                 ~/.local/share/obrain-harness/bin/obrain
cp ~/sauvegardes-bascule-v3/obrain.avant-garde-confinement-20260915 \
   ~/projects/banc-obrain-j5/target/release/obrain
systemctl --user restart obrain-viz.service
```

## 5. Ce qui reste ouvert (protocole, hors confinement)

Les 10 échecs de protocole du binaire servi sont **antérieurs** et indépendants :
`HEAD` refusé en 405, aucun en-tête `Date`, aucun `ETag`/304 sur les statiques
hachés, `Connection: close` systématique (pas de keep-alive), pas de 400 sur une
ligne de requête vide, pas de 400 sans `Host`, page d'entrée en `no-store`.
Le binaire de la lignée consolidée, lui, passe **15/15** (mesuré le 2026-09-15).
