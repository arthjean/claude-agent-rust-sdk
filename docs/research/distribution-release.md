# Distribution : ce que fait le SDK Python, ce qui existe côté Rust, ce qui reste à faire

> Date : 2026-11-17. Base : `claude-agent-sdk-rust` au commit `ee751a9` (branche `main`) ; `claude-agent-sdk-python` au commit `4720d36` (version 0.2.182, identique à l'épingle du sous-module).
> Méthode : cinq sous-agents en lecture seule, un par domaine (versionnage et release Python ; artefacts Python ; changelog, docs et durcissement Python ; état actuel côté Rust ; écosystème de release Rust sur sources officielles). Chaque rapport a été vérifié avant d'être accepté : les fichiers et lignes cités ont été relus, les faits web recoupés sur la page officielle. Un essai réel d'empaquetage a été lancé (`cargo package`, hors ligne, sortie dans `target/package`). Aucun fichier existant modifié, aucun appel payant, rien envoyé à crates.io.
> Périmètre : recherche seulement. Aucune décision n'est prise ici ; la section 6 liste celles qui reviennent au propriétaire.

Vocabulaire. *Wheel* : paquet Python prêt à installer. *sdist* : archive des sources Python. *`.crate`* : archive des sources d'une crate Rust, l'unique artefact que crates.io distribue. *OIDC* : jeton d'identité de courte durée émis par GitHub pour un job. *Trusted publishing* : le registre accepte l'envoi sur présentation de ce jeton, sans mot de passe stocké. *Yank* : retirer une version de la résolution de nouvelles dépendances sans la supprimer. *MSRV* : plus ancienne version de Rust supportée. *Attestation* : preuve signée de la façon dont un fichier a été construit.

## 1. Résumé

1. **Côté Python, la release est entièrement automatisée et pilotée par le CLI.** Un robot externe pousse un commit `chore: bump bundled CLI version to X` sur `main` ; si les tests passent, `auto-release.yml` calcule patch+1 et appelle `build-and-publish.yml`, qui construit 5 wheels (chacun embarque le binaire du CLI) et un sdist, les envoie à PyPI par trusted publishing, fait écrire le changelog par Claude, pousse le commit de version et le tag sur `main`, puis crée une GitHub Release sans fichier joint.
2. **Côté Rust, rien de tout cela n'existe**, et c'est voulu : `tasks/prd-claude-agent-rust-sdk.md:1149` a reporté « Distribution and release » à un PRD ultérieur. Toutes les crates sont en `0.1.0`, il y a 0 tag, pas de `CHANGELOG`, pas de workflow de release, et le README dit de dépendre du dépôt git.
3. **Ce que le dépôt Rust a déjà, et souvent en mieux** : une seule source pour la version, des tests de politique qui tiennent les versions épinglées en phase, des actions verrouillées par commit, un contrôle de licences, un job MSRV, un job de versions minimales, une veille des releases amont.
4. **L'essai d'empaquetage révèle quatre défauts bloquants ou gênants** : le paquet ne contient ni `LICENSE` ni `THIRD_PARTY_NOTICES` ; quatre fichiers `.profraw` non ignorés rendent l'arbre « sale » et font échouer `cargo package` ; aucun manifeste n'a de champ `repository` ; 67 fichiers de tests partent dans le paquet alors qu'ils dépendent de `fake-claude`, qui n'est pas publié.
5. **Une bonne moitié de la machinerie Python ne s'applique pas** : tout ce qui sert à embarquer le CLI (wheels par plateforme, téléchargement, recompression, quota PyPI) et tout ce qui protège les jobs qui appellent Claude en CI.
6. **Trois contraintes changent la forme de la solution Rust** : le trusted publishing de crates.io refuse le déclencheur `workflow_run` (celui de la release automatique Python) ; la première publication d'une crate doit être faite à la main ; la règle « un seul contributeur visible » interdit le commit de version poussé par un robot.

## 2. Ce que fait le SDK Python (vérifié)

Chemins relatifs à `/home/arthur/dev/claude-agent-sdk-python`.

### 2.1 Versionnage

- **Deux numéros indépendants** (`RELEASING.md:25-30`). Version du SDK : `pyproject.toml:7` et `src/claude_agent_sdk/_version.py:3`. Version du CLI embarqué : `src/claude_agent_sdk/_cli_version.py:3` (`2.1.325`). Tags `vX.Y.Z`.
- **Synchronisation par script** : `scripts/update_version.py:16-40` réécrit les deux copies par expression régulière, sans valider l'entrée. Aucun test ne compare les deux fichiers : ils restent en phase parce que la CI de release les réécrit ensemble.
- **Épingle du CLI** : `scripts/update_cli_version.py:22-59`, validée par `scripts/_cli_version_validation.py:30,92-119` (version concrète exigée, `latest` refusé). `scripts/build_wheel.py:105-126` échoue plutôt que de retomber sur `latest`.
- **Relation entre les deux** : aucune numériquement. Chaque montée du CLI produit un patch du SDK (`RELEASING.md:34`).

### 2.2 Déclenchement et orchestration

- **Release automatique** : `auto-release.yml:3-15` se déclenche par `workflow_run` quand `Test` réussit sur un push de `main` dont le message commence par `chore: bump bundled CLI version to`. Il vérifie que `_cli_version.py` a changé (`:24-29`), calcule patch+1 (`:31-38`), puis appelle le workflow réutilisable avec `contents: write`, `id-token: write`, `secrets: inherit` (`:46-56`).
- **Qui pousse le commit de montée du CLI** : un robot hors du dépôt. `git log` donne l'auteur `GitHub Actions <actions@github.com>` pour ces commits, et aucun workflow du dépôt ne les produit.
- **Release manuelle** : `publish.yml:3-9` (`workflow_dispatch`, version en texte libre). Elle relance pytest sur Python 3.10 à 3.13 (`:12-33`) et ruff plus mypy (`:35-58`) avant d'appeler le même workflow (`:75-85`). La version saisie n'est pas validée.
- **Workflow réutilisable** `build-and-publish.yml` : quatre jobs en chaîne, `build-wheels` (`:17-43`), `publish` (`:45-230`), `changelog` (`:235-244`), `release` (`:246-324`).
- **Commit, tag, GitHub Release** : le job `release` tourne dans l'environnement `production` avec une clé de déploiement (`:250-260`), commite sous l'identité `github-actions[bot]` (`:269-277`), pousse directement sur `main` sans PR (`:309-312`), crée un tag annoté (`:318-319`), puis une GitHub Release dont les notes sont la section du changelog extraite par `awk`, plus un lien PyPI et une ligne `pip install` (`:321-324`). Aucun fichier n'est joint.
- **Ordre** : PyPI est servi avant le commit de version et le tag. Si le job `release` échoue, PyPI porte une version sans tag (*inférence*, rien ne réconcilie). Aucune clé `concurrency` dans `.github`.

### 2.3 Authentification et garde-fous

- **Trusted publishing** : `twine upload` sans jeton, dans l'environnement `production`, avec `id-token: write` (`build-and-publish.yml:48-52,220-230` ; `RELEASING.md:80-86`). PyPI fait confiance au workflow qui exécute le job, pas à ses appelants.
- **Présence des 5 wheels** vérifiée par étiquette de plateforme (`:73-90`).
- **Contrôle de quota avant envoi** (`:101-218`) : 250 Mio par fichier, 150 Gio par projet, arrêt au-delà de 95 %. Un wheel trop gros est écarté avec un avertissement ; un sdist trop gros ou PyPI injoignable arrête tout.
- **Veille quotidienne du quota** : `pypi-quota-check.yml:3-18`, `scripts/check_pypi_quota.py:20-21,89-120` (alerte à 80 %, message Slack, job en échec).
- **Reprise** : `--skip-existing` rend une relance sûre après un envoi partiel (`:221-230`). Pas de retour arrière ni de yank automatisé.
- **Alertes d'échec** : `release-alert.yml:7-25,51-55,74-79` poste sur Slack quand `Test` échoue sur une montée du CLI ou quand une release échoue.
- **Absences vérifiées** : pas de pré-version, de version candidate, de nightly ni de branche de maintenance.

### 2.4 Artefacts

- **Deux sortes** : un sdist et cinq wheels (`macosx_11_0_arm64`, `macosx_11_0_x86_64`, `manylinux_2_17_x86_64`, `manylinux_2_17_aarch64`, `win_amd64`), construits sur cinq machines (`build-and-publish.yml:21`).
- **Construction** : hatchling (`pyproject.toml:1-3`). Le wheel pur est réétiqueté par plateforme quand le binaire est présent (`scripts/build_wheel.py:158-221,479-495`). Si le réétiquetage échoue, le script avertit seulement.
- **Contenu du sdist** : `/src`, `/tests`, `/scripts`, `/examples`, `README.md`, `LICENSE` (`pyproject.toml:66-77`). Pas de binaire.
- **Contenu d'un wheel** : le paquet, `py.typed`, `_bundled/claude[.exe]`, `_bundled/claude.version`.
- **Obtention du CLI** : `scripts/download_cli.py:250-277` télécharge `https://claude.ai/install.sh` (ou `install.ps1`) et l'exécute. Le seul contrôle porte sur la forme du script (`:129-160`). **Aucune somme de contrôle ni signature du binaire**, et la version que le binaire annonce n'est pas comparée à l'épingle (`:308-331`).
- **Taille** : recompression zopfli de chaque wheel, avec vérification que noms, ordre, contenus, dates et droits sont inchangés (`build_wheel.py:243-249,428-453`).
- **Vérification sur PR** : `build-wheel-check.yml:5-20` construit sur les cinq machines, vérifie l'étiquette (`:60-70`), la présence du binaire (`:72-94`), installe et importe (`:96-101`). Un second job vérifie que la matrice de PR et celle de release sont identiques (`:109-124`).
- **Vérification à la release** : `twine check` n'émet que des avertissements (`build_wheel.py:514-544`). Les wheels construits ne sont pas testés.
- **Outils de build épinglés par hash** : `scripts/requirements-build.txt:1-6`, `scripts/requirements-recompress.txt`, installés avec `--require-hashes --only-binary :all:` (`build-and-publish.yml:31-32`).
- **Versions supportées** : `requires-python >=3.10` et planchers de dépendances (`pyproject.toml:10,27-33`). Seul le plancher de `mcp` est testé (`test.yml:44-70`).
- **Extras** : `dev`, `otel`, `examples` (`pyproject.toml:35-55`).
- **Docker** : `Dockerfile.test` sert aux tests, jamais publié.
- **Absences vérifiées** : aucune attestation, signature ni liste de composants (recherche sur `attest|sigstore|cosign|sbom|provenance|gpg`, sans résultat pertinent).

### 2.5 Changelog

- **Format** : `# Changelog`, puis `## <version>` sans « v » ni date, puis des sections `###` (Breaking Changes, New Features, Bug Fixes, Documentation, Internal/Other Changes) (`CHANGELOG.md:1-18`, `.claude/commands/generate-changelog.md:10-17`).
- **Génération** : `generate-changelog.yml:46-66` lance `anthropics/claude-code-action@v1` avec la commande `/generate-changelog`, le modèle `claude-opus-4-6`, le mode de permission `auto` et trois outils autorisés.
- **Cloisonnement** : ce job tourne après l'envoi à PyPI, hors de l'environnement `production`, avec un jeton en lecture seule (`generate-changelog.yml:3-9,29-37`). Il ne transmet que `CHANGELOG.md`, après avoir vérifié que c'est un fichier texte simple de 1 Mio au plus, sans chaîne ressemblant à un secret (`:71-97`). Le job `release` refait ces contrôles avant de commiter (`build-and-publish.yml:286-307`).
- **Non bloquant** : `continue-on-error` (`generate-changelog.yml:48`, `build-and-publish.yml:248-249`). Effet constaté : la dernière section du changelog est `0.2.178` alors que le paquet est en `0.2.182` (`CHANGELOG.md:3`, `pyproject.toml:7`).

### 2.6 Documentation publiée et métadonnées

- **Canaux** : le README, qui est aussi la page PyPI (`pyproject.toml:9`) ; la documentation hébergée sur `platform.claude.com` (`README.md:3`) ; 16 scripts d'exemple. Aucun site de documentation n'est construit depuis le dépôt.
- **README** : pas de badge ; installation par `pip install claude-agent-sdk` ; mention de la version minimale du CLI (`README.md:95`) ; un guide de migration qui renvoie à `CHANGELOG.md#010` (`README.md:299-306`). Sa section « Release Workflow » est périmée : elle décrit une PR de release (`README.md:353-371`) alors que le code pousse directement sur `main`.
- **Métadonnées** (`pyproject.toml:5-60`) : nom, description, licence MIT, auteur, classifieurs (Alpha, Python 3.10 à 3.13, Typing :: Typed), mots-clés, trois URL (Homepage, Documentation, Issues).
- **Guide de release** : `RELEASING.md` (périmé sur un point : `:77` cite un secret `ANTHROPIC_API_KEY` qu'aucun workflow n'utilise).

### 2.7 Communauté et durcissement du pipeline

- **Tri d'issues par Claude** : `claude-issue-triage.yml:3-43`, borné par `scripts/gh.sh` et `scripts/edit-issue-labels.sh`. **Message Slack à chaque issue** : `slack-issue-notification.yml`. **Relecture de PR par Claude** : `claude-code-review.yml`.
- **Jobs qui appellent Claude** : machine avec pare-feu sortant, liste de 17 hôtes autorisés (`.github/egress-firewall.yaml:4-39`), mode de permission `auto`, jetons de courte durée au lieu d'une clé d'API. `workflow-hardening.yml` et `.github/scripts/check_workflow_hardening.py` font échouer la CI si une de ces protections manque.
- **Épinglage des actions** : faible. 3 actions sur 8 sont épinglées par commit ; `actions/checkout@v4`, `actions/setup-python@v5`, `anthropics/claude-code-action@v1` et les autres suivent une étiquette mobile.
- **Permissions** : trois workflows sans bloc `permissions` (`lint.yml`, `build-wheel-check.yml`, `pypi-quota-check.yml`).
- **Hook de contributeur** : `scripts/initial-setup.sh:12-15` installe `scripts/pre-push` (ruff seulement).
- **Absences vérifiées** : pas de `SECURITY.md`, `CONTRIBUTING`, `CODEOWNERS`, modèles d'issue, Dependabot ni Renovate.

## 3. État actuel côté Rust (vérifié)

Chemins relatifs à la racine du dépôt.

- **Versions** : `0.1.0` pour les cinq crates, héritée de `Cargo.toml:12`. `VERSION` vient de `env!("CARGO_PKG_VERSION")` (`crates/claude-agent-kit/src/lib.rs:218`). `CLI_VERSION = "2.1.325"` (`lib.rs:221`), `MINIMUM_CLI_VERSION = "2.0.0"` (`lib.rs:225`), `UPSTREAM_SDK_VERSION = "0.2.182"` (`lib.rs:228`). Les copies (README, `docs/migration.md`, `THIRD_PARTY_NOTICES`, `e2e.yml`) sont tenues en phase par `crates/claude-agent-kit/tests/workspace.rs` (`version_constants_match_the_manifest_and_the_upstream_pin`, `:517`).
- **Seule copie manuelle de la version de crate** : `version = "0.1.0"` dans la dépendance vers la crate de macros (`crates/claude-agent-kit/Cargo.toml:23`).
- **Crates publiables** : `claude-agent-kit` et `claude-agent-kit-macros`. Un test interdit `publish = false` sur la bibliothèque (`workspace.rs:46`). `fake-claude`, `parity-driver` et `xtask` sont en `publish = false`.
- **Métadonnées** : la bibliothèque a description, mots-clés, catégories, `readme = "../../README.md"`, `exclude = ["tests/workspace.rs"]` et `[package.metadata.docs.rs] all-features = true` (`crates/claude-agent-kit/Cargo.toml:3,9-12,58-59`). La crate de macros n'a que description, licence et auteur. Aucun manifeste n'a `repository`, `homepage` ni `documentation` (cargo le signale à l'empaquetage).
- **Essai d'empaquetage** (`cargo package -p claude-agent-kit-macros -p claude-agent-kit --allow-dirty --offline`, cargo 1.100.0) :
  - les deux crates s'empaquettent et compilent : macros 5 fichiers, 3,5 Kio compressés ; bibliothèque 137 fichiers, 1,6 Mio, 354 Kio compressés, loin de la limite de 10 Mo ;
  - sans `--allow-dirty`, la commande échoue : quatre fichiers `crates/claude-agent-kit/*.profraw` ne sont ni commités ni ignorés (`.gitignore` ne contient que `/target`) ;
  - le paquet ne contient ni `LICENSE` ni `THIRD_PARTY_NOTICES` (ils n'existent qu'à la racine) ;
  - 67 fichiers sous `tests/` sont inclus, alors que la suite construit `fake-claude` à partir du workspace (`tests/common/mod.rs:33-64`) : hors du dépôt, ces tests ne peuvent pas tourner ;
  - `lib.rs:212-215` inclut `../../../README.md` sous `cfg(doctest)` : ce chemin sort du paquet, donc les tests de documentation ne compilent pas depuis le `.crate` (*inférence* tirée du chemin, non exécutée).
- **Automatisation de release** : aucune. Quatre workflows (`ci.yml`, `e2e.yml`, `upstream-watch.yml`, `actions-freshness.yml`), aucun ne tague ni ne publie. Les sous-commandes de `xtask` sont `gates`, `parity-check`, `parity-report`, `parity-tests`, `migration-table`, `parity-page`, `upstream-diff`, `parity-diff`, `e2e` (`xtask/src/main.rs:38-46`). `git tag` : 0 tag.
- **Changelog** : aucun fichier `CHANGELOG*`. Les 66 commits du dépôt suivent tous la forme `type(portée): sujet` (commits conventionnels), sans qu'aucun outil ne l'impose.
- **Documentation** : le README dit « The crate is not published on crates.io yet. Depend on it from git » (`README.md:46-50`), sans badge. Il contient 7 liens relatifs vers le dépôt (`README.md:11,12,163,196,205,219,229`). Les docs de crate sont écrites pour docs.rs (`lib.rs:1-106`), et rustdoc tourne avec les avertissements refusés dans les gates. Aucun usage de `cfg(docsrs)` ni de `doc_cfg` dans les sources. `docs/migration.md` et `docs/parity.md` existent.
- **Chaîne d'approvisionnement** : actions par étiquette verrouillées par commit dans `.github/workflows/actions.lock`, vérifiées en CI (`ci.yml:262-267`) ; `permissions: contents: read` par défaut dans les quatre workflows ; `cargo deny check licenses` en CI (`deny.toml` n'a que `[graph]` et `[licenses]`, donc pas de contrôle des avis de sécurité) ; job `msrv` (`ci.yml:74`) ; job `minimal-versions` (`ci.yml:162-182`) ; `Cargo.lock` commité.
- **Veille amont** : `upstream-watch.yml` (hebdomadaire) ouvre une issue `Port upstream release <tag>` par release amont non portée (`.github/scripts/upstream-watch.sh:74-92`). Le portage est manuel.
- **Décisions déjà écrites** (`tasks/prd-claude-agent-rust-sdk.md`) :
  - `:1149` : distribution, installateur du CLI « with checksum and signature verification », pipeline de release, suivi de version du CLI et publication sur crates.io reportés à un PRD ultérieur ;
  - `:1150` : pas de CLI embarqué (limite de 10 Mo et conditions de redistribution) ;
  - `:1186` : `claude-agent-kit` est un nom de travail, à confirmer avant la première publication ;
  - `:1188` : licence GPL-3.0-or-later, avec une question ouverte sur MPL-2.0 ;
  - `:1172` : versionnage sémantique, montée mineure pour un relèvement de MSRV.

## 4. Faits sur l'écosystème Rust (sources officielles)

- **Trusted publishing sur crates.io** : disponible depuis juillet 2025 pour GitHub Actions, via `rust-lang/crates-io-auth-action@v1` et `id-token: write`. La première version d'une crate doit être publiée à la main. [Billet du 2025-07-11](https://blog.rust-lang.org/2025/07/11/crates-io-development-update-2025-07/), [RFC 3691](https://rust-lang.github.io/rfcs/3691-trusted-publishing-cratesio.html).
- **Mode « Trusted Publishing Only »** : une fois activé dans les réglages de la crate, la publication par jeton est désactivée. Les déclencheurs `pull_request_target` et `workflow_run` sont refusés. [Billet du 2026-01-21](https://blog.rust-lang.org/2026/01/21/crates-io-development-update/).
- **Limites et permanence** : 10 Mo par `.crate` ; une publication est permanente, une version ne peut être ni écrasée ni supprimée ; un yank ne supprime rien. [Cargo Book, publishing](https://doc.rust-lang.org/cargo/reference/publishing.html).
- **Publication multi-crates** : `cargo publish --workspace` est stable depuis Cargo 1.90 (2025-09-18), dans l'ordre des dépendances, sans atomicité : un échec peut laisser le workspace à moitié publié. [Changelog de Cargo](https://doc.rust-lang.org/cargo/CHANGELOG.html).
- **SemVer en 0.x** : seul le composant non nul le plus à gauche compte comme incompatible ; en `0.y.z`, `y` joue le rôle de majeur. [Référence SemVer de Cargo](https://doc.rust-lang.org/cargo/reference/semver.html).
- **docs.rs** : construit sur nightly, sans réseau, avec `--cfg docsrs`. Clés disponibles : `features`, `all-features`, `no-default-features`, `default-target`, `targets`, `rustdoc-args`, `cargo-args`. [Métadonnées docs.rs](https://docs.rs/about/metadata), [builds](https://docs.rs/about/builds). `doc_auto_cfg` a été fusionné dans `doc_cfg` ([PR rust#138907](https://github.com/rust-lang/rust/pull/138907)).
- **Provenance** : ni crates.io ni cargo ne vérifie de signature ou d'attestation aujourd'hui ; la RFC 3691 l'exclut de son périmètre. GitHub offre les attestations (`actions/attest`) et les releases immuables (disponibles depuis le 2025-10-28 : fichiers verrouillés, tag non déplaçable). [Releases immuables](https://github.blog/changelog/2025-10-28-immutable-releases-are-now-generally-available/).
- **Crate de macros compagne** : `thiserror` épingle sa crate d'implémentation par version exacte (`=2.0.21`) ; `serde` laisse `serde_derive` en dépendance lâche. Deux pratiques coexistent.
- **Outils existants** (versions relevées, fonctions non étudiées ici) : release-plz 0.4.3, cargo-release 1.1.7, git-cliff 2.14.2, cargo-semver-checks 0.51.0 (détecte les ruptures d'API ; limites déclarées sur les génériques, les features hors jeu par défaut et les cibles), cargo-dist 0.34.0.

## 5. Tableau de synthèse

État : **Existe** (rien à faire), **Partiel**, **Absent**, **N/A** (ne s'applique pas). Les lignes R1 à R4 n'ont pas d'équivalent Python.

| # | Mécanisme | Ce que fait le SDK Python (source) | État côté Rust | Travail à faire |
|---|---|---|---|---|
| 1 | Numéro de version | Deux copies, semver, tags `vX.Y.Z` (`pyproject.toml:7`, `src/claude_agent_sdk/_version.py:3`, `RELEASING.md:25-30`) | Partiel : source unique `Cargo.toml:12`, mais politique non écrite et 0 tag | Écrire la politique (0.x : le mineur est cassant ; MSRV), choisir la première version et le format de tag |
| 2 | Synchronisation des copies | `scripts/update_version.py:16-40` | Partiel : une copie manuelle dans `crates/claude-agent-kit/Cargo.toml:23` | Tenir cette copie par un test de politique ou par l'outil de release ; décider l'épinglage exact (`=`) de la crate de macros |
| 3 | Épingle de version du CLI | `_cli_version.py:3`, `scripts/update_cli_version.py`, `scripts/_cli_version_validation.py` | Existe : `CLI_VERSION` (`lib.rs:221`), copies tenues par `tests/workspace.rs:517` | Optionnel : une commande `xtask` qui déplace toutes les copies d'un coup |
| 4 | Robot de montée du CLI | Commits poussés par un robot hors dépôt (auteur `GitHub Actions`) | Partiel : `upstream-watch.yml` ouvre une issue par release amont | Décider le lien entre portage d'une release amont et release Rust |
| 5 | Release automatique | `auto-release.yml:3-56` (`workflow_run`, patch+1) | Absent | Choisir le déclencheur (tag poussé ou lancement manuel) : `workflow_run` est refusé par crates.io ; décider la cadence |
| 6 | Release manuelle | `publish.yml:3-85` (version saisie, tests et lint d'abord) | Absent | Workflow de release qui relance les gates avant de publier et valide la version |
| 7 | Chaîne construire, publier, noter, taguer | `build-and-publish.yml:17-324` | Absent | Écrire le workflow : empaqueter, publier, créer la GitHub Release ; groupe `concurrency` |
| 8 | Commit de version et tag par robot | `build-and-publish.yml:250-319` (clé de déploiement, push direct sur `main`) | Absent, et incompatible avec « un seul contributeur visible » | Commit de version et tag faits par le propriétaire ; la CI publie depuis le tag et vérifie tag = version de `Cargo.toml` ; règle de protection des tags |
| 9 | GitHub Release | `build-and-publish.yml:321-324` (notes tirées du changelog, aucun fichier joint) | Absent | Créer la Release depuis la section du changelog, avec liens crates.io et docs.rs ; décider d'y joindre les `.crate` ; activer les releases immuables |
| 10 | Environnement protégé | `environment: production` (`build-and-publish.yml:48,251`) | Absent | Environnement `release` avec approbation du propriétaire ; `id-token: write` sur le seul job de publication |
| 11 | Authentification au registre | Trusted publishing PyPI (`build-and-publish.yml:220-230`, `RELEASING.md:80-86`) | Absent | Première publication à la main avec un jeton, puis trusted publishing sur les deux crates et mode « Trusted Publishing Only » |
| 12 | Tests avant release | `publish.yml:12-58` ; `Test` réussi pour la voie automatique | Partiel : les gates existent, sans lien avec une release | Faire dépendre la publication des gates sur le commit tagué |
| 13 | Complétude des artefacts | 5 étiquettes de plateforme exigées (`build-and-publish.yml:73-90`) | N/A : pas d'artefact par plateforme | Rien ; l'équivalent est la ligne 17 |
| 14 | Quota du registre | Contrôle avant envoi (`build-and-publish.yml:101-218`), veille quotidienne (`pypi-quota-check.yml`, `scripts/check_pypi_quota.py`) | N/A : 354 Kio pour une limite de 10 Mo, pas de quota par projet | Rien ; au plus une assertion de taille dans le job d'empaquetage |
| 15 | Reprise après échec | `--skip-existing` (`build-and-publish.yml:221-230`) ; pas de yank automatisé | Absent | Rendre le job relançable (sauter une crate déjà publiée à cette version, la publication multi-crates n'étant pas atomique) ; documenter `cargo yank` |
| 16 | Alertes d'échec | `release-alert.yml` (Slack) | Absent | Optionnel : pour un mainteneur seul, la notification d'échec de GitHub suffit |
| 17 | Vérification du paquet sur PR | `build-wheel-check.yml:5-124` (construction, contenu, installation, import) | Absent | Job CI : `cargo publish --workspace --dry-run`, contrôle de la liste des fichiers (licences présentes, rien de parasite), construction de la doc comme docs.rs |
| 18 | Vérification à la release | `twine check`, avertissement seul (`scripts/build_wheel.py:514-544`) | Absent | Couvert par la ligne 17, en bloquant |
| 19 | Archive des sources | sdist (`pyproject.toml:66-77`) | Partiel : le `.crate` se construit mais est défectueux | Ajouter `LICENSE` et `THIRD_PARTY_NOTICES` dans chaque crate publiée ; liste `include` explicite ; ignorer `*.profraw` ; décider si `tests/` part dans le paquet ; corriger l'inclusion du README en test de documentation |
| 20 | Paquets par plateforme avec CLI embarqué | 5 wheels (`build-and-publish.yml:17-43`, `scripts/build_wheel.py:158-221`) | N/A : exclu par `tasks/prd-claude-agent-rust-sdk.md:1150` | Rien |
| 21 | Obtention du CLI | `scripts/download_cli.py:250-277`, sans somme de contrôle ni signature | Absent : reporté par `tasks/prd-claude-agent-rust-sdk.md:1149` | Décider : aide d'installation vérifiée (somme de contrôle, signature) ou documentation seule |
| 22 | Recompression | zopfli (`scripts/build_wheel.py:243-453`) | N/A | Rien |
| 23 | Marqueur de typage | `py.typed`, classifieur `Typing :: Typed` (`pyproject.toml:24`) | N/A | Rien |
| 24 | Outils de build épinglés | `scripts/requirements-build.txt`, `--require-hashes` (`build-and-publish.yml:31-32`) | Partiel : `Cargo.lock` commité, actions verrouillées | Épingler les outils du workflow de release à une version exacte ; publier avec `--locked` |
| 25 | Versions supportées | `requires-python`, planchers (`pyproject.toml:10,27-33`), plancher `mcp` testé (`test.yml:44-70`) | Existe : `rust-version`, jobs `msrv` et `minimal-versions` (`ci.yml:74,162`) | Rien |
| 26 | Options d'installation | Extras `dev`, `otel`, `examples` (`pyproject.toml:35-55`) | Existe : features documentées dans `lib.rs` | Décider si la feature `testing` fait partie du contrat public |
| 27 | Image Docker | `Dockerfile.test`, jamais publiée | N/A | Rien |
| 28 | Provenance et signature | Aucune (trusted publishing seul) | Absent | Aller plus loin que Python : attestation des `.crate` et releases immuables ; crates.io ne vérifie rien à ce jour |
| 29 | Fichier de changelog | `CHANGELOG.md:1-18` (sections par version, sans date) | Absent | Créer `CHANGELOG.md` : une section datée par version, avec la version amont et la version du CLI correspondantes |
| 30 | Génération du changelog | Claude en CI, non bloquant (`generate-changelog.yml:46-97`, `.claude/commands/generate-changelog.md`) ; retard constaté de 4 versions | Absent ; les 66 commits sont conventionnels | Choisir la méthode (génération depuis les commits, relue, commitée par le propriétaire avant le tag) ; rendre l'entrée obligatoire pour publier |
| 31 | Contrôles du fichier de changelog | Fichier simple, 1 Mio, pas de secret (`build-and-publish.yml:286-307`) | N/A si le changelog est écrit avant le tag | Rien |
| 32 | Page du registre | README comme page PyPI (`pyproject.toml:9`) | Partiel : `readme` pointé, mais installation par git, 7 liens relatifs, pas de badge, pas de README pour la crate de macros | Réécrire la section Installation ; vérifier le rendu des liens relatifs sur crates.io ; badges ; README de la crate de macros ; adapter les tests de `workspace.rs` qui épinglent le README |
| 33 | Documentation de référence | Hébergée ailleurs (`README.md:3`), aucun site construit | Partiel : docs prêtes pour docs.rs, `all-features = true` | Afficher la feature requise par item (`doc_cfg`, `rustdoc-args`) ; vérifier la construction façon docs.rs en CI |
| 34 | Métadonnées du registre | `pyproject.toml:5-60` | Partiel : `repository`, `homepage`, `documentation` absents ; crate de macros presque vide | Compléter les deux manifestes |
| 35 | Exemples | 16 scripts, inclus dans le sdist | Existe : 12 exemples, inclus dans le paquet | Rien |
| 36 | Notes de migration et de rupture | `README.md:299-306` vers `CHANGELOG.md#010` | Partiel : `docs/migration.md` couvre Python vers Rust, pas les montées de version Rust | Section « versions et compatibilité » dans le README ; ruptures signalées dans le changelog |
| 37 | Guide de release | `RELEASING.md` | Absent | Écrire `RELEASING.md` : procédure, première publication, secrets, yank |
| 38 | Pré-versions, nightly, maintenance | Aucun (absence vérifiée) | Absent | Décider seulement si la première publication passe par une version candidate |
| 39 | Tri d'issues, Slack, relecture par Claude | `claude-issue-triage.yml`, `slack-issue-notification.yml`, `claude-code-review.yml` | Absent | Hors distribution ; chaque job coûterait des crédits d'API. À ne pas reprendre sans décision |
| 40 | Politique de sécurité, guide de contribution | Aucun (absence vérifiée) | Absent | Aller plus loin que Python : `SECURITY.md` avant la première publication |
| 41 | Épinglage des actions | Faible : 3 actions sur 8 par commit | Existe, plus strict : `actions.lock` vérifié en CI (`ci.yml:262-267`) | Faire passer les actions du workflow de release par `gh actions-lock` |
| 42 | Permissions des workflows | Inégal : 3 workflows sans bloc | Existe : `contents: read` par défaut | Garder ce défaut dans le workflow de release |
| 43 | Protections des jobs qui appellent Claude | `.github/egress-firewall.yaml`, `workflow-hardening.yml`, jetons de courte durée | N/A : aucun job n'appelle Claude en CI | Rien, tant que la ligne 39 reste écartée |
| 44 | Mise à jour des dépendances | Aucune automatisation | Partiel : `actions-freshness.yml` pour les actions, licences contrôlées | Optionnel : contrôle planifié des avis de sécurité (`cargo deny check advisories`) |
| 45 | Hook de contributeur | `scripts/pre-push`, `scripts/initial-setup.sh` | N/A : `cargo xtask gates` remplit ce rôle | Rien |
| R1 | Nom et licence définitifs | Sans équivalent | Décidé le 2026-11-18 (section 6) : `claude-agent-rust-sdk`, `MIT OR Apache-2.0` | Renommer les deux crates et le dépôt ; changer de licence dans les manifestes, les fichiers de licence, `deny.toml`, le README et les tests de politique |
| R2 | Ordre de publication de deux crates | Sans équivalent (un seul paquet) | Absent | Publier la crate de macros avant la bibliothèque (`cargo publish --workspace` le fait) |
| R3 | Contrôle de compatibilité d'API | Sans équivalent | Absent | Job `cargo-semver-checks` contre la dernière version publiée, à partir de la deuxième release |
| R4 | Tests de politique du dépôt | Sans équivalent | Existe, à étendre | Ajouter : la première section du changelog égale la version de `Cargo.toml` ; forme du workflow de release (jamais sur les forks, environnement, permissions) |

## 6. Décisions qui reviennent au propriétaire

1. Nom définitif de la crate et licence (R1), avant toute publication.
2. Cadence : une release Rust par release amont portée, ou des releases regroupées (lignes 4 et 5).
3. Rapport entre la version Rust et la version amont : numérotation indépendante avec correspondance écrite dans le changelog, ou autre schéma (ligne 1).
4. Aide d'installation du CLI vérifiée, ou documentation seule (ligne 21).
5. Changelog généré depuis les commits ou rédigé à la main (ligne 30).
6. Contenu du paquet : avec ou sans `tests/` (ligne 19).
7. Feature `testing` : contrat public ou détail interne (ligne 26).

### Décisions prises le 2026-11-18

- **Nom de la crate : `claude-agent-rust-sdk`** (crate de macros : `claude-agent-rust-sdk-macros`), en remplacement du nom de travail `claude-agent-kit`. Le dépôt est à renommer à l'identique pour éviter l'inversion avec `claude-agent-sdk-rust`.
  - Vérifié sur l'API de crates.io les 2026-11-17 et 2026-11-18 (réponse 404, donc nom libre ; la liste des noms réservés par le registre n'a pas été consultée) : `claude-agent-rust-sdk`, `claude-agent-rust-sdk-macros`, `claude-agent-kit`, `claude-agent-rust`, `rust-claude-agent-sdk`, `claude-agent-sdk-port`, `claude-agent-client`.
  - Pris : `claude-agent-sdk` (0.1.1, dernière mise à jour 2025-09-30), `claude-agent-sdk-rust` (1.0.0, 543 téléchargements), `claude-agent-sdk-rs`, `claude-agent`, `claude-agent-rs`, `claude-agents`, `claude-agents-sdk`, `claude-sdk`, `claude-sdk-rs`, `claude-sdk-rust`, `claude-code-sdk`, `claude-code-sdk-rs`, `claude-code-sdk-rust`, `claude-code-agent-sdk`, `claude-cli-sdk`, `anthropic-agent-sdk`, `cc-sdk`, `cc-agent-sdk`.
  - Un nom n'est acquis qu'à la première publication.
- **Licence : `MIT OR Apache-2.0`**, en remplacement de `GPL-3.0-or-later`. Motifs : les sept crates concurrentes vérifiées sont sous MIT (`anthropic-agent-sdk`, `claude-agent-sdk-rs`, `claude-code-agent-sdk`, `cc-sdk`, `claude-code-sdk-rust`, `claude-agent-sdk`, `claude-agent-sdk-rust`) ; les 163 dépendances du workspace sont toutes sous licence permissive ; Rust lie statiquement, donc la GPL exclut toute application fermée distribuée. Fichiers touchés : `Cargo.toml`, `LICENSE` (deux fichiers de licence), `THIRD_PARTY_NOTICES`, `deny.toml`, le paragraphe de licence du README et les tests de `workspace.rs` qui l'épinglent.
- **Cadence : une release Rust par release amont portée, publiée dès que le portage est fait.** But du propriétaire : un SDK maintenu, à jour avec l'officiel en quelques heures à quelques jours. Mesures prises dans le dépôt Python le 2026-11-18 : 13 à 23 releases amont par mois de juin à octobre 2026 (`git log --grep='^chore: release v'`) ; 102 sections du changelog sur 183 ne contiennent que la montée du CLI embarqué. Conséquences : la release doit tenir en une commande locale plus un tag ; les montées du CLI seules doivent se porter par une commande mécanique ; la veille amont, hebdomadaire aujourd'hui (`.github/workflows/upstream-watch.yml:10`), doit devenir quotidienne.
- **Version : numérotation indépendante en `0.y.z`**, avec la version amont correspondante dans chaque section du changelog. Calquer le numéro amont ne laisserait pas de place à un correctif propre à Rust entre deux versions officielles.
- **Par défaut, sans objection du propriétaire** : changelog généré depuis les commits, relu et commité par le propriétaire avant le tag (décision 5) ; paquet sans `tests/`, avec une liste `include` explicite (décision 6) ; feature `testing` publique mais déclarée instable (décision 7) ; installateur vérifié du CLI hors du PRD de distribution (décision 4).
- **Reste ouverte** : lancer ou non la suite de bout en bout contre le vrai CLI à chaque release (environ 0,80 USD par exécution).

## 7. Limites de cette recherche

- **Non vérifié** : la liste des noms réservés par crates.io ; le rendu des liens relatifs du README sur crates.io ; les règles de protection de l'environnement `production` côté Python (réglages du dépôt, invisibles de l'extérieur) ; ce que vérifie `install.sh` lui-même.
- **Non étudié** : le fonctionnement détaillé de release-plz, cargo-release, git-cliff et cargo-dist (seules leurs versions ont été relevées) ; le trusted publishing à travers un workflow réutilisable ; les règles de crates.io sur la réservation de noms ; l'attestation d'un fichier `.crate` par `actions/attest`.
- **Corrections apportées aux rapports des sous-agents** : la liste du pare-feu Python compte 17 hôtes et non 18 ; le job `release` de `build-and-publish.yml` commence à la ligne 246 et non 250.
- **Effet de bord de l'essai** : deux fichiers `.crate` ont été écrits dans `target/package/` (répertoire ignoré par git).
