# Administration IAM — les comptes, enfin administrables (2026-09-13)

*Chantier mesuré sur des magasins d'identités **jetables** (`/tmp`) et un serveur
armé sur port libre. Portes : `scripts/acceptation-iam.sh` (bout en bout CLI +
HTTP, **tout passe**) et `cargo test -p obrain-iam --all-features` (**139 tests**).
Rien n'a été mesuré sur les services de l'utilisateur (8432/8433) — vérifié
intacts après coup.*

---

## 1. Le défaut, et il était double

### 1.1 La bibliothèque savait tout faire, et rien ne l'exposait

`IamStore` portait déjà `create_user`, `create_role`, `create_policy`,
`attach_role`, `attach_policy_to_role`, `deactivate_user`, `detach_role`,
`delete_role`, `delete_policy`, et `ObrainIamProvider` portait
`set_user_password`, `reemettre_session`, `revoquer_jeton`. Mais :

| surface | ce qu'elle exposait |
|---|---|
| `examples/amorcer_iam.rs` | **un** utilisateur, un rôle, deux politiques — et rien pour un second compte |
| HTTP (`viz_serve.rs`) | `/api/auth/{login,logout,whoami,refresh,audit}` **seulement** |
| CLI `obrain` | **aucune** sous-commande `iam` |

Ajouter un collègue supposait donc d'écrire un programme. C'est ce qui a été
constaté à l'usage, en demandant les identifiants du 8432 : le premier compte
existait, le deuxième était hors de portée.

### 1.2 Le défaut trouvé en chemin : **supprimer ne retirait aucun droit**

C'est le plus grave, et il a été trouvé par un test, pas par lecture attentive.

`delete_policy` et `delete_role` marquent `iam_status = "deleted"` — depuis
toujours. Mais `node_to_policy` et `node_to_role` **ne lisaient jamais ce
champ**. Conséquence, mesurée :

* `list_policies` / `list_roles` continuaient de rendre l'objet « supprimé » ;
* `get_role_policies` et `get_user_roles` aussi ;
* or `evaluate_access` collecte les politiques **en passant par les rôles** ;

donc **une politique ou un rôle « supprimé » continuait d'accorder ses droits**.
Le test qui l'a révélé est celui du retrait d'espace : après
`retirer_espace`, la session dont l'espace venait d'être fermé **passait
toujours** (`retirer_un_espace_mord_sans_purge_de_session`).

Correctif : les deux fonctions rendent `None` sur `status == "deleted"`, et le
statut **absent** vaut actif — les objets écrits avant ce correctif n'en portent
pas, et les invalider tous serait une panne. Deux tests de régression tiennent la
correction au niveau de la bibliothèque :
`store::tests::supprimer_une_politique_retire_le_droit` et
`supprimer_un_role_retire_le_droit`.

C'est la classe de défaut que ce dépôt combat déjà ailleurs : **une opération qui
répond « fait » sans rien faire**.

---

## 2. Ce qui a été ajouté

**Un seul noyau, trois surfaces** — pour qu'une règle ne puisse pas diverger
entre deux chemins.

| pièce | fichier | rôle |
|---|---|---|
| Noyau | `crates/obrain-iam/src/admin.rs` | toutes les opérations, avec leurs garanties |
| CLI | `crates/obrain-cli/src/commands/iam.rs` | hors bande, sur un fichier `.obrain` |
| HTTP + page | `crates/obrain-cli/src/viz_serve.rs` | plafonné par les droits de l'appelant |
| Porte | `scripts/acceptation-iam.sh` | vérifie les deux surfaces, y compris leurs refus |

Le noyau ne connaît qu'un `Arc<IamStore>` : il ne lit aucun fichier, ne dépend
d'aucune couche, et se teste sans disque ni réseau. Ouvrir une base reste le
travail de l'appelant — le CLI ouvre le `.obrain`, le serveur a déjà le sien.

### Le CLI — hors bande

```text
obrain iam amorcer <base> <espace> [--utilisateur admin] [--mot-de-passe X]
obrain iam utilisateurs|utilisateur|ajouter|desactiver|reactiver
obrain iam mot-de-passe|espace-ajouter|espace-retirer
obrain iam admin-ajouter|admin-retirer
obrain iam jetons|jeton|jeton-revoquer
obrain iam roles|politiques|politique-supprimer
obrain iam audit|exporter|effacer --oui
```

L'autorité du CLI vient du **système d'exploitation** : il agit sur un fichier,
et prétendre le contraire serait une comédie. Il ne se plafonne donc pas — il se
déclare (`est_hors_bande()`).

### Le serveur — plafonné, et jugé sur un espace **nommé**

Treize routes (liste, détail, inventaires, piste, export, et huit gestes), toutes
gardées par l'action NOMMÉE `iam:admin` **au point de contrôle unique** qui
couvrait déjà les routes de données — une route d'administration ajoutée demain
est donc gardée sans qu'on ait à y penser. Plus une page `/iam`, servie en HTML.

Deux décisions qui méritent d'être dites :

* **`/api/iam/moi`** exige une identité, mais aucun droit : on n'y lit que
  **soi**. Sans elle, la page devrait *deviner* sur quel espace se juger — ou se
  fier à un espace qu'elle fournit, c'est-à-dire exactement ce que la garde
  refuse de faire. C'est cette route qui rend l'écran possible sans le rendre
  permissif.
* **Un espace nommé, jamais un défaut.** Une route d'administration sans
  `espace=` (ou champ `espace` au corps) est refusée en **400** : la juger sur
  l'espace par défaut du serveur reviendrait à trancher sur un espace que la
  requête ne vise pas — soit on refuse un geste légitime, soit on l'autorise au
  titre d'un autre. Un `?espace=` n'a **pas** été ajouté à `espace_vise`, qui
  sert aussi à `/api/query` : là, il laisserait un client choisir l'espace sur
  lequel on le juge, et donc se faire juger sur un espace qu'il détient pour en
  lire un autre.

Verdict de la porte (`scripts/acceptation-iam.sh`, port 8439) :

```text
1) binaire
  ✅ la sous-commande « iam » est COMPILÉE (elle répond, au lieu de s'annoncer incompilée)
2) amorçage d'un administrateur
  ✅ amorçage par le CLI · ✅ l'administrateur se lit dans la liste
3) gestes du CLI
  ✅ ajout d'un compte (mot de passe engendré rendu une fois)
  ✅ nom dangereux refusé (« a/b ») · ✅ désactivation d'un compte
  ✅ effacement refusé sans --oui · ✅ effacement exécuté avec --oui
4) serveur armé (port 8439)          ✅ garde armée
5) garde et plafond
  ✅ administration sans jeton → 401
  ✅ administration sans espace nommé → 400 (nommer, jamais défaut)
  ✅ « qui suis-je » lisible avec une simple identité
  ✅ liste des comptes avec cookie → 200
  ✅ création hors du plafond → 403 (l'espace « hors-portee » n'est pas détenu)
  ✅ création dans le plafond → 200
  ✅ désactivation par GET → 405 (une mutation ne s'obtient pas d'un lien)
  ✅ désactivation par POST → 200
  ✅ désactivation du dernier administrateur refusée
  ✅ l'administrateur est toujours joignable (verrouillage tenu)
6) page et piste
  ✅ page /iam servie
  ✅ la page ne détient aucune donnée : elle interroge l'API gardée
  ✅ piste d'audit des gestes d'administration lisible
7) verdict                            ADMINISTRATION IAM COMPLÈTE — tout passe.
```

La page `/iam` est une **coquille sans données**, à dessein : `/iam` n'étant pas
sous `/api/`, la garde globale ne la couvre pas, et une page qui rendrait la
liste des comptes côté serveur serait **publique**. La coquille n'appelle que
l'API gardée ; un inconnu qui l'ouvre ne lit que du vide et un 401.

---

## 3. Les trois garanties

### 3.1 Le plafond se **lit**, il ne se déclare pas

Une surface réseau qui fournirait elle-même son plafond pourrait se décrire plus
large que ses droits : la règle « on n'accorde pas ce qu'on ne détient pas »
serait alors appliquée à partir d'une donnée fournie par celui qu'elle contrôle.

`Administration::plafonnee_par_l_acteur` **déduit** donc le plafond des
politiques de l'acteur, dans le magasin. Conséquence de conception assumée : le
**défaut est fermé**. Un acteur sans politique obtient un plafond *vide*, donc
tout geste est refusé — et non « tout est permis tant qu'on n'a rien dit ». Un
compte non actif est refusé d'emblée.

Quatre tests le tiennent : `le_plafond_se_deduit_des_droits_de_lacteur`,
`un_acteur_sans_droit_obtient_un_plafond_vide`,
`un_compte_ferme_ne_fournit_pas_de_plafond`, `un_acteur_inconnu_ne_fournit_pas_de_plafond`.

### 3.2 Aucun verrouillage possible

Le dernier administrateur actif ne peut être ni désactivé, ni dégradé, ni
effacé, et sa politique d'administration ne peut pas être supprimée — trois
chemins distincts, tous gardés. Sans cela, un geste légitime ferme la porte à
clé derrière soi, et il ne reste que l'édition du fichier à la main.

Mesuré depuis le CLI, sur le compte `admin` amorcé :

```text
$ obrain iam desactiver <base> admin
error: access denied: refus de désactiver « admin » : c'est le dernier
administrateur actif. Promouvoir un autre compte d'abord.

$ obrain iam admin-retirer <base> admin demo
error: access denied: refus de dégrader l'administration de « admin » : c'est
le dernier administrateur actif.

$ obrain iam politique-supprimer <base> p-admin-demo-admin
error: access denied: refus de supprimer « p-admin-demo-admin » : elle porte le
dernier accès d'administration actif.
```

Et le verrou n'est pas un mur : après `admin-ajouter second demo`, la
désactivation de `admin` **passe** — c'est la preuve que la garde compte les
administrateurs, et ne bloque pas par excès de prudence.

### 3.3 Tout geste est tracé, sur l'action qu'il a faite

Chaque opération écrit une entrée d'audit sur l'action NOMMÉE réellement
accomplie — `iam:user:create`, `iam:user:disable`, `iam:space:revoke`,
`iam:gdpr:erase`, … — et non sur un `iam:admin` générique. Le **principal est
l'acteur**, la **ressource est le compte visé** : une piste où l'acteur et la
cible se confondent ne répond pas à « qui a fait ça ? ».

```text
HORODATAGE            ACTION            ACTEUR  CIBLE  DÉCISION
--------------------  ----------------  ------  -----  --------
2026-09-13T23:20:18Z  iam:user:create   cli     alice  accordé
2026-09-13T23:20:18Z  iam:user:disable  cli     alice  accordé
2026-09-13T23:20:01Z  iam:bootstrap     cli     admin  accordé
```

### 3.4 Une grammaire de noms, fermée

Le nom entre dans un ORN
(`orn:obrain:graph:obrain:tenant/<espace>`), dans un identifiant de nœud
(`u-<nom>`), dans un nom de cookie et dans la piste d'audit. Un `/` y découpe un
chemin, un `:` y sépare deux champs, un `*` y devient un joker, `..` y ressemble
à une remontée de chemin. On n'accepte donc qu'une **grammaire fermée** —
minuscules ASCII, chiffres, `.`, `_`, `-`, commençant par une lettre ou un
chiffre — plutôt que de courir après une liste noire.

```text
$ obrain iam ajouter <base> 'a/b' --espace demo
error: invalid parameter: utilisateur « a/b » : il sépare ou découpe un ORN
```

### 3.5 On n'administre que ce qu'on détient — la portée de la **cible**

Le plafond limitait ce qu'un administrateur peut **accorder**. Il ne disait rien
de ce qu'il peut **atteindre** : un administrateur du seul espace `e1` pouvait
désactiver, changer le mot de passe ou effacer un compte qui ne vit que dans
`e2`. Il n'obtenait pas `e2` — mais il pouvait en fermer l'accès. C'est un
pouvoir de nuisance hors de sa portée, et le taire aurait laissé croire que
« administrateur d'un espace » veut dire quelque chose.

On n'administre donc que les comptes dont on détient **déjà tous les espaces**.
Deux tests le tiennent (`on_nadministre_pas_un_compte_hors_de_sa_portee`,
`on_administre_un_compte_dans_sa_portee`).

Toutes les opérations publiques passent par **un seul guichet** qui vérifie cette
portée, pour qu'aucune — y compris une opération ajoutée demain — ne puisse
l'oublier. Ce guichet a d'ailleurs révélé une récursion au passage : `resumer`
a besoin des jetons, et la vérification de portée a besoin de `resumer`. La
lecture des jetons a donc été séparée du contrôle qui garde l'entrée
(`jetons_de`), et c'est documenté à l'endroit où ça se produit.

---

## 4. Décisions de conception, et leurs raisons

**Le plafond ne se contourne pas par l'administration.** `emettre_jeton` et
`amorcer` refusent d'être appelés sur une administration plafonnée : fabriquer un
jeton pour autrui serait un contournement de la connexion, et l'amorçage pose
`iam:admin` — l'autoriser sur une surface réseau reviendrait à laisser toute
surface fabriquer son propre administrateur. Ces deux gestes restent hors bande
(le CLI). Deux tests le vérifient.

**Promouvoir est un geste distinct de créer.** Un compte neuf ne reçoit aucun
droit d'administration : les espaces demandés, `graph:read`/`graph:write` dessus,
`iam:audit` dessus. On ne devient pas administrateur par accident en créant un
compte.

**Un mot de passe lu une fois doit être changé.** `changer_mot_de_passe` pose
`must_change_password` : un mot de passe qu'un administrateur a vu n'est pas un
mot de passe que l'utilisateur a choisi.

**Aucun mot de passe ni jeton n'est relisible.** Le magasin n'en garde que
l'empreinte (bcrypt pour les mots de passe, SHA-256 pour les jetons). Les listes
de jetons rendent identifiant et échéance, **jamais la valeur**. Seuls les gestes
qui en engendrent un (ajout, changement de mot de passe, émission de jeton)
l'impriment, une fois, et le disent.

**La désactivation n'a pas besoin de purger les sessions** — et ce n'est pas une
promesse de ce module, c'est une propriété de la chaîne, vérifiée dans le code :
`authenticate_by_token_hash` relit l'utilisateur à **chaque** requête et refuse
si `status != Active`, et `authorize` réévalue les politiques à chaque requête.
Un test le mesure contre la vraie garde
(`un_jeton_d_un_compte_desactive_est_refuse`) et un autre pour le retrait
d'espace (`retirer_un_espace_mord_sans_purge_de_session`).

---

## 5. Trois pièges trouvés en chemin, et ce qu'ils ont appris

**Un contrôle indirect valide ce qu'il ne mesure pas.** La porte vérifiait
l'embarquement de l'IAM par `nm -C binaire | grep obrain_iam`. Mesuré le
2026-09-14 : un binaire construit **sans** la feature portait quand même des
symboles `obrain_iam` (une autre dépendance du workspace la tire), donc la sonde
passait au vert pendant que la commande répondait « IAM NON COMPILÉ ». La porte
fait désormais **parler la commande** et lit son verdict. C'est la même famille
que la ligne de construction documentée le 13/09 : un `cargo build` ordinaire
sur le même `target/` remplace le binaire complet, et trois vérifications ont
ainsi été perdues dans ce dépôt.

**Un commentaire qui contredit le code ne protège de rien.** Le garde-méthode
interdisait `GET` sur `/api/iam/utilisateurs` — alors que le commentaire juste
au-dessus affirmait le contraire (« la liste n'est pas dans cette liste : lire
n'est pas un geste »). La porte a rendu **trois 405** là où elle attendait 401,
400 et 200. Le code est corrigé, et le commentaire dit maintenant pourquoi cette
route n'y est pas. Sans exécution, la contradiction serait passée.

**Un espace par défaut juge une requête sur autre chose qu'elle-même.** D'où le
400 explicite quand l'espace n'est pas nommé, plutôt qu'un défaut silencieux.

---

## 6. Ce qui reste hors de la chaîne

* **Pas d'écran dans l'application React** (`packages/obrain-dashboard`) : la page
  `/iam` est servie par le serveur, sans JavaScript. Un écran React demanderait
  de reconstruire le bundle, pour un gain qui n'est pas là — les formulaires
  suffisent aux gestes, et la page reste utilisable sans build.
* **Pas de MFA, pas de renouvellement automatique côté client, pas d'énumération
  des sessions d'une identité** — inchangé depuis la note IAM du 13/09.
* **Pas de renommage de compte** : l'identifiant sert de clé d'ORN, de nom de
  cookie et de principal d'audit. Un renommage casserait la piste.
* **La révocation d'un jeton dépend encore de la visibilité de `delete_node` dans
  le substrat** : `revoke_session` écrit une expiration dans le passé *et* tente
  la suppression, précisément parce que la suppression s'est déjà révélée
  insuffisante sur une base fichier. Les tests acceptent les deux issues, et le
  disent.
* **Défaut PRÉEXISTANT, constaté et NON corrigé ici** (il vit dans `gdpr.rs`, hors
  du périmètre de ce chantier) : le certificat d'effacement et l'export RGPD
  portent un horodatage **gelé** — mesuré : `erased_at: 2026-01-01T00:00:01Z`,
  `exported_at: 2026-01-01T00:00:00Z` — et `principal` y désigne la **cible** au
  lieu de l'acteur, alors que la piste d'audit, elle, porte la vraie date et le
  vrai acteur. C'est exactement la famille du défaut que la note IAM du 13/09
  célébrait avoir corrigé (« avant, `now_iso()` était un compteur, donc l'audit
  n'aurait daté aucun incident ») : un certificat non daté ne date rien. Le
  signaler vaut mieux que le laisser croire à une preuve.
* **L'effacement RGPD anonymise, il ne supprime pas** : le nœud du compte
  subsiste en `ERASED`, et deux comptes effacés donnent deux lignes
  indiscernables. Les libellés du CLI et de l'API disent « anonymisé », pas
  « supprimé », pour ne rien promettre de faux.
