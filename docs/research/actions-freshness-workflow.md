# Workflow de fraîcheur des actions : est-il utile ? Comparaison avec le dépôt du SDK Python

> Date : 2026-11-16. Base : ce dépôt au commit `c3ec762` (branche `main`) ; `claude-agent-sdk-python` au commit `4720d36`, qui est toujours la tête de son `main` amont (contrôlé par `git ls-remote`).
> Méthode : un seul agent, sans sous-agent. Fichiers locaux et historique git lus directement. Documentation GitHub lue par son point d'accès Markdown (`docs.github.com/api/article/body`). Sources des outils lues à un commit figé (le SHA est dans chaque lien). Trois essais en lecture seule : le script de fraîcheur lancé localement, `gh actions-lock --no-fix`, et les réglages du dépôt lus par l'API GitHub. Aucun workflow lancé, aucun appel payant. Cette note s'appuie sur `docs/research/ci-open-decisions.md` (section 4) et ne répète pas ce qu'elle tranche. Ce qui n'a pas pu être confirmé est marqué « non vérifié » et repris en section 7.

## 1. Verdict

**Le garder, avec trois corrections. Ne pas le remplacer, ne pas le supprimer.**

1. **Il ne fait double emploi avec rien dans ce dépôt.** Le fichier de verrouillage (fichier qui enregistre le commit exact de chaque action) fige chaque action, et l'outil qui le gère n'a aucun rapport « une release plus récente existe ». Sa documentation confie ce travail à Dependabot (le robot de mise à jour de GitHub), et Dependabot est écarté ici parce qu'une PR Dependabot fusionnée laisse le robot comme auteur d'un commit (`docs/research/ci-open-decisions.md`, section 4.1).
2. **Le dépôt Python n'est pas un modèle à copier.** Il n'a aucune automatisation des mises à jour, et son historique en montre le résultat : pas une seule version d'action n'a été relevée en 17 mois, et ses références ont aujourd'hui deux à quatre versions majeures de retard.
3. **Tel qu'il est écrit, le workflow a des lacunes qui comptent** : il n'a aucune voie pour les correctifs de sécurité, il peut annoncer « tout est à jour » alors qu'une requête a échoué, et il saute en silence certaines formes de référence. La section 6 liste les corrections.

Vocabulaire. *Action* : étape réutilisable publiée dans un autre dépôt et appelée par `uses: owner/name@ref`. *Étiquette déplaçable* : étiquette (tag) comme `v1` que son éditeur déplace à chaque nouvelle release. *Cooldown* : délai d'attente avant d'adopter une nouvelle release. *Pull request (PR)* : changement proposé, en attente de fusion.

## 2. Ce que fait le workflow de fraîcheur (vérifié)

- **Déclencheur et droits.** Il s'exécute le premier jour de chaque mois à 06:17 UTC et sur lancement manuel (`.github/workflows/actions-freshness.yml:4-6`), avec un accès en lecture seule au dépôt (`:8-9`). Il récupère le code et lance un seul script (`:19-24`).
- **Détection, étiquettes de version complète.** Le script relève chaque ligne `uses: owner/name@ref` des workflows (`.github/scripts/actions-freshness.sh:25-26`). Pour une version complète comme `v7.0.1`, il demande à l'API GitHub les releases de l'action, garde celles publiées depuis au moins 14 jours, et signale la plus haute si elle est plus récente que la référence actuelle (`:13-14`, `:33-46`).
- **Détection, étiquettes déplaçables.** Pour toute autre référence, il compare le commit enregistré dans le fichier de verrouillage au commit que la référence désigne aujourd'hui, et signale un écart dès que le nouveau commit a au moins 14 jours (`:47-63`). Une seule référence est dans ce cas : `dtolnay/rust-toolchain@v1` (`.github/workflows/actions.lock:30-32`).
- **Où atterrit le résultat.** Chaque constat est affiché dans le journal du job et ajouté à la page de résumé de l'exécution (`:18-23`). Le script sort avec le code 1 dès qu'il y a au moins un constat (`:70`), donc l'exécution est marquée en échec. Il n'ouvre ni issue ni PR, et ne modifie aucun fichier.
- **Qui est prévenu.** Une exécution planifiée en échec prévient une seule personne : l'utilisateur qui a créé le workflow, ou le dernier à avoir modifié sa ligne `cron`, sous réserve des réglages de notification de cet utilisateur ([notifications for workflow runs](https://docs.github.com/en/actions/concepts/workflows-and-actions/notifications-for-workflow-runs)).
- **Ce qu'un humain doit faire ensuite.** Modifier à la main chaque ligne `uses:` listée, lancer `gh actions-lock` (ou `gh actions-lock --relock` pour une étiquette déplaçable), puis commiter les workflows avec le fichier de verrouillage (`:9-10`, `:61`).
- **État à ce jour.** Une seule exécution existe, manuelle, le 2026-11-16, verte en 13 secondes (`gh run list`). La première exécution planifiée est prévue le 2026-12-01. Le script lancé localement aujourd'hui affiche « Every action is on its newest release older than 14 days » et sort avec le code 0.

Rien d'autre dans le dépôt ne surveille l'arrivée de releases plus récentes. Le seul autre contrôle sur les actions est le job `lockfile`, qui lance `gh actions-lock --verify` à chaque PR et à chaque push (`.github/workflows/ci.yml:187-204`) et teste l'intégrité, pas l'âge. Il n'y a ni `dependabot.yml` ni configuration Renovate (le dossier `.github/` contient trois workflows, le fichier de verrouillage et le script).

## 3. Dépôt Rust et dépôt Python, côte à côte (vérifié)

| | Dépôt Rust (`c3ec762`) | Dépôt Python (`4720d36`) |
|---|---|---|
| Comment les actions sont référencées | Étiquettes lisibles dans les workflows : cinq versions complètes et une étiquette déplaçable (`.github/workflows/actions.lock:7-18`). Chacune est fixée à un commit par le fichier de verrouillage (`:19-49`) | Étiquettes majeures déplaçables pour `actions/*` et `anthropics/claude-code-action` (`.github/workflows/test.yml:17`, `test.yml:20`, `claude-code-review.yml:34`). Un SHA de commit complet avec un commentaire de version pour trois actions (`test.yml:39`, `pypi-quota-check.yml:41`, `.github/actions/setup-claude-auth/action.yml:15`) |
| Contrôle d'intégrité | `gh actions-lock --verify` à chaque PR et à chaque push (`ci.yml:187-204`) | Aucun sur les références d'actions |
| Automatisation des mises à jour | Le rapport de fraîcheur mensuel | **Aucune.** Pas de `dependabot.yml`, pas de fichier Renovate, et il n'y en a jamais eu : `git log --all` sur ces chemins est vide |
| Comment arrivent les montées de version | À la main, après une exécution mensuelle rouge | **Elles n'arrivent pas.** Voir ci-dessous |
| Auteurs robots | Aucun autorisé (un seul contributeur visible) | 0 commit de Dependabot ou de Renovate sur 976 ; les 70 commits sous `.github/` sont tous écrits par des personnes |

Ce que l'historique Python montre pour les versions d'actions (`git log -S` sur chaque référence, sous `.github/`) :

- `actions/checkout@v4` et `actions/setup-python@v5` sont là depuis le premier commit (`6ca3514`, 2025-06-12) et n'ont jamais changé.
- Le seul changement de version d'une référence existante en 17 mois est le passage de `anthropics/claude-code-action@beta` à `@v1` (`f70b572`, 2026-01-12, écrit par une personne).
- Le commit d'épinglage par SHA (`3c08a3a`, 2026-05-06) a figé les versions déjà utilisées et n'en a relevé aucune.

Retard actuel des références Python (dernières étiquettes lues par `git ls-remote` le 2026-11-16) :

| Action | Dépôt Python | Dernière release | Écart |
|---|---|---|---|
| `actions/checkout` | `v4` | v7.0.1 | 3 versions majeures |
| `actions/setup-python` | `v5` | v7.1.0 | 2 versions majeures |
| `actions/upload-artifact` | `v4` | v7.0.2 | 3 versions majeures |
| `actions/download-artifact` | `v4` | v8.0.2 | 4 versions majeures |
| `codecov/codecov-action` | v4.6.0 (SHA) | v7.1.1 | 3 versions majeures |
| `slackapi/slack-github-action` | v2.1.1 (SHA) | v4.0.0 | 2 versions majeures |
| `actions/github-script` | v9.0.0 (SHA) | v9.0.0 | à jour |

Lecture : à la question « comment tiennent-ils les actions à jour », le dépôt Python répond « ils ne le font pas ». Le workflow de fraîcheur fait plus que l'amont, il n'y a donc rien en amont sur quoi s'aligner.

## 4. Autre chose fait-il déjà ce travail ?

### 4.1 L'outil de verrouillage lui-même : non (vérifié)

Le mécanisme est le fichier de verrouillage des dépendances de workflow de GitHub : `.github/workflows/actions.lock`, généré par l'extension `gh actions-lock` de l'outil en ligne de commande de GitHub (`actions.lock:1-4`), disponible pour tous depuis le 2026-10-28 ([annonce](https://github.blog/changelog/2026-10-28-github-actions-workflow-lockfiles-are-generally-available/)).

- Sa propre réponse pour les nouvelles releases est Dependabot : « Let Dependabot handle version bumps » et « We recommend letting Dependabot manage your GitHub Actions dependencies » ([guide, étapes 1 et 5](https://docs.github.com/en/actions/how-tos/secure-your-work/lock-workflow-dependencies)).
- Aucun de ses modes ne liste les releases plus récentes : génération, `--relock`, `--verify`, `--verify-local`, `--no-fix` ([README L51-L59](https://github.com/github/gh-actions-lock/blob/90ae563599cf/README.md?plain=1#L51-L59) ; `gh actions-lock --help`, v0.4.1, lu localement).
- Une étiquette déplaçable qui a bougé garde son commit verrouillé, et l'avertissement `ref-moved` « appears only in `--json` output and doesn't change the exit code » ([dépannage L255-L261](https://github.com/github/gh-actions-lock/blob/90ae563599cf/docs/troubleshooting.md?plain=1#L255-L261)). Le job `--verify` de `ci.yml` ne montre donc rien quand `dtolnay/rust-toolchain@v1` bouge.
- Sa seule notion de cooldown est un avis, `fresh-tag`, affiché quand une étiquette en cours de verrouillage a été publiée dans les trois derniers jours (ou dans le cooldown de Dependabot s'il y en a un de configuré). Il « doesn't hold back a release » ([dépannage L297-L303](https://github.com/github/gh-actions-lock/blob/90ae563599cf/docs/troubleshooting.md?plain=1#L297-L303)).

Le constat de `docs/research/ci-open-decisions.md` (section 4.3 F) tient donc toujours : le fichier de verrouillage règle l'intégrité, pas la fraîcheur.

### 4.2 Dependabot : oui, et il comprend désormais le fichier de verrouillage (vérifié)

- **La prise en charge du fichier de verrouillage est native.** « When Dependabot opens a version update for an action in a workflow you've onboarded, it also regenerates the matching lockfile entry in the same pull request », sans configuration supplémentaire ([guide, étape 5](https://docs.github.com/en/actions/how-tos/secure-your-work/lock-workflow-dependencies), [guide d'application](https://docs.github.com/en/actions/how-tos/administer/enforce-workflow-lockfiles)). Le code source le confirme : Dependabot lance `gh-actions-lock` sur les workflows modifiés ([file_updater.rb L57-L95](https://github.com/dependabot/dependabot-core/blob/da6dfa3b074a/github_actions/lib/dependabot/github_actions/file_updater.rb#L57-L95), [cli_engine.rb L58-L80](https://github.com/dependabot/dependabot-core/blob/da6dfa3b074a/github_actions/lib/dependabot/github_actions/lockfile/cli_engine.rb#L58-L80)) et accepte le format de fichier de verrouillage utilisé ici, `v0.0.3` ([constants.rb L34-L40](https://github.com/dependabot/dependabot-core/blob/da6dfa3b074a/github_actions/lib/dependabot/github_actions/constants.rb#L34-L40)).
- **Ce qu'il met à jour.** Les références écrites `owner/name@tag` ou `owner/name@<commit>`, ainsi que le commentaire de version sur la même ligne ([écosystèmes pris en charge, GitHub Actions](https://docs.github.com/en/code-security/dependabot/ecosystems-supported-by-dependabot/supported-ecosystems-and-repositories#github-actions)).
- **Cadence.** `monthly` s'exécute le premier jour de chaque mois ; une expression `cron` est aussi acceptée ([référence des options, `schedule`](https://docs.github.com/en/code-security/dependabot/working-with-dependabot/dependabot-options-reference)).
- **Cooldown.** Trois jours par défaut, même sans configuration, réglable par `cooldown.default-days`. Pour GitHub Actions, les réglages par niveau (majeur, mineur, correctif) ne sont pas pris en charge. Le cooldown ne s'applique jamais aux mises à jour de sécurité (même page, `cooldown`).
- **Regroupement.** `groups` réunit toutes les mises à jour correspondantes dans une seule PR (même page, `groups`).
- **Étiquettes déplaçables.** Le guide laisse un `main` ou un `v3` qui a bougé à un `gh actions-lock --relock` manuel (étape 5.3). Si une exécution de Dependabot en fait jamais avancer une : non vérifié.

Dependabot fait donc tout ce que fait le script pour les versions complètes, et écrit en plus le changement. Le workflow de fraîcheur réimplémente sa moitié détection, sans la PR. La seule raison de ne pas l'utiliser est la règle « un seul contributeur visible », qui tient toujours : quelle que soit la méthode de fusion, le robot reste l'auteur d'un commit sur la branche par défaut (`docs/research/ci-open-decisions.md`, section 4.1).

### 4.3 Renovate (vérifié, dans la mesure où cela change le verdict)

Renovate régénère lui aussi `actions.lock` quand il met à jour une action, en lançant lui-même `gh actions-lock`, et marque cette prise en charge comme expérimentale ([readme L65-L95](https://github.com/renovatebot/renovate/blob/ac5b1a9fe6d6/lib/modules/manager/github-actions/readme.md?plain=1#L65-L95)). Il ne touche pas aux étiquettes déplaçables qui ont bougé (mêmes lignes). Il ne change pas le verdict : l'application hébergée écrit des commits en tant que robot, et la forme auto-hébergée demande un jeton personnel de longue durée (`docs/research/ci-open-decisions.md`, section 4.3 B).

## 5. L'angle sécurité

### 5.1 Un rapport mensuel n'est pas un canal de sécurité

- **Vérifié.** Quand une action a une vulnérabilité publiée, les alertes Dependabot préviennent le dépôt, et les mises à jour de sécurité Dependabot ouvrent une PR vers la plus basse version corrigée ([référence d'usage sécurisé](https://docs.github.com/en/actions/reference/security/secure-use), [mises à jour de sécurité](https://docs.github.com/en/code-security/concepts/supply-chain-security/dependabot-security-updates)). Les alertes se déclenchent quand un avis de sécurité est ajouté à la GitHub Advisory Database, pas selon un calendrier ([alertes Dependabot](https://docs.github.com/en/code-security/concepts/supply-chain-security/dependabot-alerts)).
- **Vérifié.** « For GitHub Actions, alerts are only generated for actions that use semantic versioning, not SHA versioning » (même page). Les workflows d'ici nomment des étiquettes de version (`ci.yml:29`, `ci.yml:36`), la forme dont les alertes ont besoin.
- **Vérifié.** Les deux fonctions sont désactivées dans ce dépôt aujourd'hui : l'API renvoie 404 pour `vulnerability-alerts` et `"enabled": false` pour `automated-security-fixes`.
- *Inférence.* Avec le seul workflow de fraîcheur, le correctif d'une action vulnérable est mentionné pour la première fois jusqu'à environ six semaines après sa publication : 14 jours de cooldown, puis jusqu'à un mois avant l'exécution suivante. Il apparaît comme une montée de version ordinaire, sans mention d'une vulnérabilité.
- *Inférence.* Activer les alertes ajoute un signal de sécurité sans commit de robot ni PR de robot, puisqu'une alerte est un avis dans l'onglet sécurité du dépôt. Que les alertes fonctionnent aussi pour un workflow couvert par un fichier de verrouillage découle des deux énoncés ci-dessus mais n'est écrit nulle part : non vérifié.

### 5.2 Le cooldown et l'incident tj-actions

- **Vérifié.** Lors de la compromission de `tj-actions/changed-files` (14 au 15 mars 2025), des attaquants « retroactively modified multiple version tags to reference a malicious commit » ([avis GHSA-mrrh-fwg8-r2c3](https://github.com/advisories/GHSA-mrrh-fwg8-r2c3)).
- **Vérifié.** Le fichier de verrouillage arrête déjà cette classe d'attaque : « The locked commit is what runs », et une étiquette de version complète qui ne pointe plus sur son commit verrouillé est une erreur ([concept](https://docs.github.com/en/actions/concepts/security/workflow-dependency-lockfiles), [guide, étape 5.3](https://docs.github.com/en/actions/how-tos/secure-your-work/lock-workflow-dependencies)).
- *Inférence.* Le cooldown couvre un autre risque : adopter une nouvelle release malveillante dès l'origine, avant que quiconque l'ait remarqué. Les 14 jours du script sont plus stricts que les 3 jours par défaut de Dependabot, et correspondent à ce que configurent crossbeam et cargo-llvm-cov (`docs/research/ci-open-decisions.md`, section 4.4). Dependabot peut être réglé sur la même valeur, le cooldown n'est donc pas un avantage du script.
- **Vérifié.** Le cooldown ne contraint que le rapport. Rien n'empêche de verrouiller à la main une release toute fraîche : `taiki-e/install-action@v2.90.3` a été publiée le 2026-11-08 (API GitHub) et épinglée le 2026-11-15 (`f833527`), à l'intérieur de la fenêtre de 14 jours du script lui-même.

## 6. Alternatives, avec ce que chacune coûte et apporte

| Option | Gain | Coût | Issue |
|---|---|---|---|
| **Garder le workflow, corrigé** | Le seul signal de fraîcheur qui ne laisse de robot nulle part. Couvre les étiquettes déplaçables, qu'aucun autre outil ne signale de façon visible | 70 lignes de shell à maintenir. Un humain modifie chaque ligne `uses:` à la main | **Recommandé** |
| Mises à jour de version Dependabot, PR fusionnées | Automatisation complète, fichier de verrouillage régénéré dans la même PR, regroupement, cooldown, rien à maintenir | Le robot devient auteur d'un commit sur `main` | Exclu par la règle du contributeur |
| Dependabot comme simple signal : PR laissée non fusionnée, changement refait localement | Un diff prêt, testé par la CI, fichier de verrouillage compris | Le robot est visible comme auteur de PR sur un dépôt public. Une PR non fusionnée par mois. Si la PR se ferme d'elle-même une fois le changement fait à la main : non vérifié | Seulement si un robot auteur de PR est acceptable (section 7) |
| Alertes Dependabot, en complément | Un signal de sécurité dans le délai d'un avis de sécurité, sans PR ni commit | Un réglage du dépôt | **Ajout recommandé** |
| Renovate, auto-hébergé, avec le jeton du propriétaire | Commits écrits sous l'identité du propriétaire, fichier de verrouillage régénéré | Un jeton de longue durée avec les droits `repo` et `workflow`, stocké comme secret dans un dépôt public. Prise en charge du fichier de verrouillage marquée expérimentale | Disproportionné pour six actions |
| Supprimer, comme le fait le dépôt Python | Rien à maintenir | Le fichier de verrouillage masque le vieillissement. Le dépôt Python a deux à quatre versions majeures de retard après 17 mois | Rejeté |

Trois corrections du script, chacune liée à un défaut vérifié :

1. **Une requête en échec compte comme « à jour ».** La requête des releases se termine par `|| true` (`actions-freshness.sh:35-38`). Un essai avec un dépôt qui n'existe pas renvoie un résultat vide, et le script continue sans un mot. Une limite de débit ou une action renommée produirait le même effet.
2. **Certaines références ne sont jamais contrôlées.** Le motif de la ligne 25 ne reconnaît que `owner/name@ref`. Un essai montre qu'il saute une action placée dans un sous-dossier (`owner/name/path@ref`) et une référence entre guillemets. Aucune n'existe aujourd'hui, la lacune est donc latente. Une action qui publie des étiquettes sans releases GitHub est elle aussi invisible pour la branche des versions complètes (`:35`).
3. **Aucune voie de sécurité.** Activer les alertes Dependabot (section 5.1).

Un point plus mineur. Le script lit le fichier de verrouillage avec `grep` et `sed` (`:51`). La commande de l'outil lui-même, `gh actions-lock --no-fix --json`, rapporte le même constat `ref-moved` sous une forme structurée (section 4.1), sans le cooldown.

## 7. Questions ouvertes et limites

**À trancher par le propriétaire :**

- **Un robot visible seulement comme auteur de PR enfreint-il la règle « un seul contributeur visible » ?** Si non, « Dependabot comme signal » devient un vrai remplaçant. `docs/research/ci-open-decisions.md` (section 5.3) a laissé ce point ouvert, et le dépôt est désormais public, ce qui rend la liste des PR publique elle aussi.
- **Une exécution rouge chaque mois est-elle acceptable ?** `taiki-e/install-action` a publié 13 releases en octobre 2026 (API GitHub). *Inférence* : une fois un mois écoulé, il existera presque toujours une release plus vieille que 14 jours et plus récente que la version épinglée, donc le job échouera à la plupart des exécutions du premier du mois. Il fonctionne alors comme un rappel de calendrier accompagné d'une liste, pas comme un signal d'exception.
- **Qui surveille l'outil de verrouillage lui-même ?** L'extension est installée à une étiquette fixe, `--pin v0.4.1` (`ci.yml:199`). Ni le fichier de verrouillage ni le script ne la couvrent.

**Vérifié depuis la note précédente :**

- Les runners standard sont gratuits pour les dépôts publics ([facturation](https://docs.github.com/en/billing/concepts/product-billing/github-actions)), donc l'argument de coût de la note précédente ne s'applique plus.
- « In a public repository, scheduled workflows are automatically disabled when no repository activity has occurred in 60 days » ([événements, `schedule`](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#schedule)). Une période calme de deux mois couperait le rappel sans prévenir.

**Non vérifié :**

- Si les alertes Dependabot se déclenchent pour un workflow couvert par un fichier de verrouillage (la documentation n'énonce la règle que pour les étiquettes de version face aux épinglages par SHA).
- Si Dependabot ferme sa PR quand la même mise à jour est faite à la main, et s'il fait jamais avancer une étiquette déplaçable qui a bougé, comme `@v1`.
- Si la notification d'exécution en échec atteint le propriétaire : cela dépend des réglages de notification personnels, et aucune exécution planifiée n'a encore eu lieu.
- Une exécution rouge du script : aucune action n'est en retard aujourd'hui, donc seul le chemin vert a été observé. Les deux défauts de la section 6 ont été montrés par des essais isolés, pas par une exécution complète en échec.
- La prise en charge du fichier de verrouillage par Renovate a été lue dans sa documentation, pas exécutée. Cette documentation décrit encore la fonction GitHub comme une préversion publique, elle est donc en retard sur l'annonce de disponibilité générale.

**Confiance :** élevée sur les sections 2, 3 et 4, lues à la source. Moyenne sur la section 5.1, où le bénéfice des alertes pour un workflow verrouillé repose sur une inférence.
