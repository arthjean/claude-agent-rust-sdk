# Décisions CI ouvertes : couverture, versions minimales, actions épinglées

> Date : 2026-11-15. Base : arbre de travail non commité de `.github/workflows/ci.yml` (branche `main`, après `b70c87a`) ; `claude-agent-sdk-python` au commit `4720d36`.
> Méthode : un seul agent, sans sous-agent. Fichiers locaux lus directement. Sources web téléchargées puis lues par extraits : documentation officielle, README et sources des outils, fichiers CI réels figés à un commit (le SHA est dans chaque lien). API GitHub publique, sans authentification, pour les commits, contributeurs et releases. Aucune commande du projet lancée, aucun appel payant. Ce qui n'a pas pu être vérifié est marqué « non vérifié » et repris en section 6.

## 1. Résumé

1. **Couverture.** Le dépôt Python ne bloque rien sur la couverture : il envoie un rapport à un service externe et ignore les erreurs d'envoi. Parmi les projets Rust lus, aucun n'utilise un seuil fixe local comme barrière de fusion ; ceux qui se donnent une barrière (rustls, le gabarit jonhoo) passent par un cliquet (plancher qui ne peut que monter) hébergé chez Codecov. Un chiffre de couverture détecte les tests supprimés ou sautés, jamais les tests affaiblis : seul le test de mutation les détecte.
2. **Versions minimales.** Les deux drapeaux Cargo sont toujours instables (nightly seulement), et la doc de Cargo déconseille `-Z minimal-versions` au profit de la variante `direct`. Le job peut passer au rouge sans changement du dépôt, par construction. Une variante déterministe existe (fichier de verrouillage minimal commité, pratiquée par rust-bitcoin).
3. **Actions épinglées.** Quelle que soit la méthode de fusion, une PR Dependabot ou Renovate (application hébergée) laisse le robot comme auteur d'un commit sur la branche par défaut, donc dans la liste des contributeurs. Les seules voies compatibles avec la règle « un seul contributeur visible » sont celles où le commit est écrit sous l'identité du propriétaire : outil en ligne de commande lancé localement, Renovate auto-hébergé avec le jeton du propriétaire, ou le fichier de verrouillage natif de GitHub Actions (`gh actions-lock`, disponible pour tous depuis le 2026-10-28), que la doc de GitHub place désormais au-dessus de l'épinglage par SHA.

## 2. Couverture : un chiffre doit-il bloquer une fusion ?

Vocabulaire. *Couverture* : part du code exécutée au moins une fois par les tests. *Région* : segment de code entre deux points de branchement, plus fin qu'une ligne. *Cliquet* (ratchet) : plancher qui ne peut que monter. *Couverture du patch* : couverture des seules lignes ajoutées ou modifiées par un changement. *Test de mutation* : un outil altère le code (par exemple remplace `+` par `-`) et vérifie qu'au moins un test échoue.

### 2.1 Ce que fait le dépôt Python (vérifié)

- Il mesure et envoie, sans seuil. `pytest --cov=claude_agent_sdk --cov-report=xml` (`.github/workflows/test.yml:36`), puis envoi à Codecov avec `fail_ci_if_error: false` (`test.yml:38-42`, et `test.yml:72-79` pour le second job avec le drapeau `mcp-v1-floor`).
- Aucun seuil nulle part : pas de `--cov-fail-under`, pas de section `[tool.coverage]` dans `pyproject.toml` (les seules options pytest sont en `pyproject.toml:79-85`), aucun fichier `codecov.yml`, `.codecov.yml` ou `.coveragerc` dans le dépôt (recherche par nom sur tout l'arbre).
- Aucun jeton Codecov n'est passé à l'action (`test.yml:39-42` : seules les clés `file`, `flags`, `fail_ci_if_error`).
- Sur le commit de tête `4720d36`, l'API GitHub renvoie 0 statut de commit et 39 exécutions de checks, toutes émises par `github-actions` : Codecov n'a rien publié sur ce commit ([statuts](https://api.github.com/repos/anthropics/claude-agent-sdk-python/commits/4720d367f7db9d5df8a166c74d7f36a538ac8c34/status), [checks](https://api.github.com/repos/anthropics/claude-agent-sdk-python/commits/4720d367f7db9d5df8a166c74d7f36a538ac8c34/check-runs?per_page=100)).
- Le seul autre contrôle est une relecture par Claude dont le prompt cite « Test coverage » parmi cinq sujets (`.github/workflows/claude-code-review.yml:49-55`) : un avis, pas une barrière.

*Inférence* : la couverture y est au mieux consultée à la main. L'envoi échoue peut-être en silence (l'action est en v4.6.0, sans jeton, et l'échec est ignoré) ; non vérifié, car les journaux et les règles de protection de branche ne sont pas visibles de l'extérieur.

### 2.2 Ce qu'offre cargo-llvm-cov (vérifié, README au commit `dc92794`, version 0.9.1 du 2026-09-06)

| Besoin | Option | Source |
|---|---|---|
| Seuil global en pourcentage | `--fail-under-lines`, `--fail-under-functions`, `--fail-under-regions` | [README L167-L177](https://github.com/taiki-e/cargo-llvm-cov/blob/dc927946e73a/README.md?plain=1#L167-L177) |
| Seuil par fichier | `--fail-under-file-lines` (ajouté en 0.8.6) | [README L173-L174](https://github.com/taiki-e/cargo-llvm-cov/blob/dc927946e73a/README.md?plain=1#L173-L174), [CHANGELOG L42](https://github.com/taiki-e/cargo-llvm-cov/blob/dc927946e73a/CHANGELOG.md?plain=1#L42) |
| Plafond en nombre absolu de lignes, régions ou fonctions non couvertes | `--fail-uncovered-lines`, `--fail-uncovered-regions`, `--fail-uncovered-functions` | [README L179-L186](https://github.com/taiki-e/cargo-llvm-cov/blob/dc927946e73a/README.md?plain=1#L179-L186) |
| Lister les lignes non couvertes | `--show-missing-lines` | [README L188-L189](https://github.com/taiki-e/cargo-llvm-cov/blob/dc927946e73a/README.md?plain=1#L188-L189) |
| Sorties exploitables par un script | `--json --summary-only`, `--lcov`, `--cobertura`, `--codecov` | [README L64-L122](https://github.com/taiki-e/cargo-llvm-cov/blob/dc927946e73a/README.md?plain=1#L64-L122) |

Ce qui manque ou demande nightly (la chaîne de compilation quotidienne, non stabilisée) :

- **Pas de seuil par crate.** Seuls existent le total et le fichier. Pas de seuil de branches non plus : la CI de l'outil porte un `# TODO: --fail-under-branches?` ([ci.yml L155](https://github.com/taiki-e/cargo-llvm-cov/blob/dc927946e73a/.github/workflows/ci.yml#L155)). Un seuil par crate par appels séparés de `report` avec `-p` : non vérifié.
- **Doctests** (exemples de code dans la documentation, exécutés comme tests) : désactivés par défaut, « nightly-only features are required » ([README L397](https://github.com/taiki-e/cargo-llvm-cov/blob/dc927946e73a/README.md?plain=1#L397)), et le support « is unstable and has known issues » ([L917](https://github.com/taiki-e/cargo-llvm-cov/blob/dc927946e73a/README.md?plain=1#L917)). Le job actuel tourne sur stable (`ci.yml:151-154`) : les doctests ne comptent donc pas.
- **Couverture de branches** (`--branch`) et MC/DC : nightly ([L203-L210](https://github.com/taiki-e/cargo-llvm-cov/blob/dc927946e73a/README.md?plain=1#L203-L210)).
- **Exclure du code** : `cfg(coverage)` est toujours posé par l'outil, `cfg(coverage_nightly)` seulement sous nightly, et l'attribut `#[coverage(off)]` est instable ([L564-L589](https://github.com/taiki-e/cargo-llvm-cov/blob/dc927946e73a/README.md?plain=1#L564-L589)). Sur stable, la seule exclusion est par nom de fichier (`--ignore-filename-regex`), plus les exclusions par défaut : dossiers `tests`, fichiers `tests.rs`, `*_tests.rs` ([L610](https://github.com/taiki-e/cargo-llvm-cov/blob/dc927946e73a/README.md?plain=1#L610)).
- **Plateformes** : sous Windows, les cibles `msvc` x86 fonctionnent ; `x86_64-pc-windows-gnu` et `aarch64-pc-windows-msvc` sont connues pour ne pas fonctionner avec la configuration par défaut au 2025-12-30 ([L614-L618](https://github.com/taiki-e/cargo-llvm-cov/blob/dc927946e73a/README.md?plain=1#L614-L618)).

*Inférence sur le job actuel* : il ne mesure que sous Linux (`ci.yml:141-143`). Le code sous `#[cfg(windows)]` ou propre à macOS n'est pas compilé, donc ni compté comme couvert ni comme manquant. Fusionner plusieurs systèmes demande un outil tiers : neqo le fait par des « flags » Codecov par plateforme ([.codecov.yml](https://github.com/mozilla/neqo/blob/90d9aec8dd77/.codecov.yml#L10-L18), [check.yml L164-L170](https://github.com/mozilla/neqo/blob/90d9aec8dd77/.github/workflows/check.yml#L164-L170)).

### 2.3 Seuil fixe, cliquet, couverture du patch

| Barrière | Ce qu'elle garantit | Sans service hébergé, sur dépôt privé ? |
|---|---|---|
| Seuil fixe | Le total ne passe pas sous N % | Oui : `cargo llvm-cov report --fail-under-lines N` |
| Cliquet | Le total ne baisse pas par rapport à la branche de base | Oui, à écrire soi-même : plancher dans un fichier du dépôt, ou `--fail-uncovered-lines`, comparé à la valeur de la branche de base (*inférence* : montage non testé ici) |
| Couverture du patch | Les lignes nouvelles sont exécutées par un test | Oui, avec un outil local : `diff-cover lcov.info --compare-branch=origin/main --fail-under=N` lit le format LCov ([README L25-L26, L196, L241-L245](https://github.com/Bachmann1234/diff_cover/blob/51b21a73f48b/README.rst?plain=1#L25-L26), v11.0.2) |

Les versions hébergées :

- **Codecov** : le statut `project` a par défaut `target: auto`, qui compare à la couverture du commit de base, avec une tolérance `threshold` ; le statut `patch` ne mesure que les lignes touchées ; `informational: true` rend le statut non bloquant ([doc des statuts](https://docs.codecov.com/docs/commit-status)). Offre gratuite « Developer » : 1 utilisateur, dépôts privés autorisés, 250 envois pour dépôts privés (période non relevée) ([tarifs](https://about.codecov.io/pricing/)). Le contenu du rapport quitte le dépôt privé.
- **GitHub natif** : la règle « Restrict code coverage » bloque une PR sous un minimum ou au-delà d'une baisse maximale par rapport à la branche par défaut ([doc](https://docs.github.com/en/code-security/how-tos/maintain-quality-code/restrict-code-coverage)). Elle ne lit que la couverture de lignes d'un rapport Cobertura ([référence](https://docs.github.com/en/code-security/reference/code-quality/code-coverage)), exige GitHub Code Quality (facturé par contributeur actif, [doc](https://docs.github.com/en/code-security/concepts/code-quality/code-quality)), est en préversion publique et annoncée pour Team et Enterprise Cloud ([annonce du 2026-06-30](https://github.blog/changelog/2026-06-30-github-code-coverage-merge-protection-for-pull-requests/), [du 2026-09-18](https://github.blog/changelog/2026-09-18-manage-the-code-coverage-ruleset-condition-with-the-rest-api/)). Disponibilité pour un compte personnel : non vérifiée, et absente de ces deux annonces.

Deux points de cadre :

- **« Bloquer » suppose une protection de branche.** Sur un dépôt privé, branches protégées et rulesets (jeux de règles de dépôt) demandent GitHub Pro, Team ou Enterprise ([branches protégées](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches), [rulesets](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/about-rulesets)). Sans cela, un job rouge informe mais n'empêche pas la fusion.
- **L'endroit où vit le seuil compte.** Un seuil écrit dans `ci.yml` ou dans un fichier du dépôt est modifiable par la PR qu'il juge. Un seuil dans les réglages du dépôt (ruleset) ou chez un service ne l'est pas.

### 2.4 Ce que font des projets Rust (vérifié sur leurs fichiers)

| Projet | Mesure | Ce qui peut bloquer | Source |
|---|---|---|---|
| rustls | cargo-llvm-cov sous nightly avec `--branch`, envoi à Codecov (`fail_ci_if_error: false`) | Statuts Codecov `patch` et `project`, tolérance 0,05 % : cliquet et patch hébergés | [build.yml L296-L324](https://github.com/rustls/rustls/blob/2cd8d27ab991/.github/workflows/build.yml#L296-L324), [codecov.yml](https://github.com/rustls/rustls/blob/2cd8d27ab991/.github/codecov.yml#L1-L8), [admin/coverage](https://github.com/rustls/rustls/blob/2cd8d27ab991/admin/coverage) |
| jonhoo/rust-ci-conf (gabarit CI réutilisé par ses crates) | cargo-llvm-cov sur stable, Codecov (`fail_ci_if_error: true`) | Statut `project`, tolérance 1 % | [test.yml L116-L164](https://github.com/jonhoo/rust-ci-conf/blob/0de345fbe62c/.github/workflows/test.yml#L116-L164), [codecov.yml](https://github.com/jonhoo/rust-ci-conf/blob/0de345fbe62c/.github/codecov.yml#L2-L12) |
| time | cargo-llvm-cov, Codecov | Rien : `informational: true` | [build.yaml L646-L671](https://github.com/time-rs/time/blob/3ce104839109/.github/workflows/build.yaml#L646-L671), [codecov.yaml](https://github.com/time-rs/time/blob/3ce104839109/.github/codecov.yaml#L10-L13) |
| quinn | cargo-llvm-cov, Codecov | Rien : `patch: off`, `project: off` | [codecov.yml](https://github.com/quinn-rs/quinn/blob/c7b46a969cd3/codecov.yml#L1-L5), [workflow](https://github.com/quinn-rs/quinn/blob/c7b46a969cd3/.github/workflows/codecov.yml#L16-L35) |
| nextest | cargo-llvm-cov sous nightly, doctests inclus, Codecov | Rien : le job est en `continue-on-error: true` (« nightly rust might break from time to time ») | [coverage.yml L26-L27, L60-L71](https://github.com/nextest-rs/nextest/blob/724cd7f904d8/.github/workflows/coverage.yml#L26-L71) |
| clap | cargo-tarpaulin, envoi à la couverture native de GitHub (`actions/upload-code-coverage`) | Non visible (dépend d'un ruleset) | [ci.yml L266-L289](https://github.com/clap-rs/clap/blob/54605ceeee4d/.github/workflows/ci.yml#L266-L289) |

Aucun job de couverture dans le fichier CI principal de tokio, hyper, serde, anyhow, crossbeam, reqwest, tower, axum et rand (recherche de `llvm-cov`, `tarpaulin`, `codecov`, `grcov` dans chaque fichier). Les seuls usages de `--fail-under-lines` trouvés dans un projet connu sont les auto-tests de cargo-llvm-cov lui-même ([ci.yml L97-L127](https://github.com/taiki-e/cargo-llvm-cov/blob/dc927946e73a/.github/workflows/ci.yml#L97-L127)). Une recherche web n'a remonté que de petits dépôts : non retenus comme preuve.

Lecture : la pratique dominante est « mesurer et afficher ». Quand un projet bloque, c'est sur une baisse (cliquet) ou sur le patch, pas sur un pourcentage fixe, et toujours via un service hébergé.

### 2.5 Quand le code et les tests sont écrits par des agents

| Défaillance | Seuil fixe | Cliquet | Couverture du patch | Test de mutation | Autre mécanisme |
|---|---|---|---|---|---|
| Test supprimé | Seulement si le total passe sous N | Oui, si le test était seul à couvrir des lignes | Non (rien n'est ajouté) | Oui, sur le code désormais non vérifié | Inventaire des tests : `xtask parity-check` vérifie déjà que chaque test amont listé a un chemin Rust existant (`xtask/src/parity/mod.rs:311-317`) |
| Test affaibli (assertion retirée, valeur attendue alignée sur le bug) | Non | Non | Non | **Oui, seul mécanisme** | Relecture humaine du diff des tests |
| Test sauté (`#[ignore]`, `#[cfg(...)]`, retour anticipé) | Comme « supprimé » | Comme « supprimé » | Non | Oui | Le résumé de `cargo test` compte les tests ignorés ; `--list` donne l'inventaire ([rustc book](https://doc.rust-lang.org/rustc/tests/index.html)) ; le lint clippy `ignore_without_reason` (groupe pedantic, depuis 1.88) impose une raison écrite ([source](https://github.com/rust-lang/rust-clippy/blob/c96aca17f5db/clippy_lints/src/attrs/mod.rs#L292-L316)) |
| Test qui exécute sans rien vérifier | Non | Non | Non (la ligne est « couverte ») | **Oui, seul mécanisme** | - |
| Barrière abaissée (seuil réduit, `COVERAGE_IGNORE` élargi, `ci.yml:145-147`) | Non | Oui, si le plancher est comparé à la branche de base ou vit hors du dépôt | Idem | Idem | Relecture humaine de tout diff sous `.github/` |

Sources et réserves :

- cargo-mutants pose la distinction : la couverture dit « what code is *reached* by a test, and not whether the test really *checks* anything » ([README L15-L18](https://github.com/sourcefrog/cargo-mutants/blob/6c720ae82a17/README.md?plain=1#L15-L18), v27.1.0).
- Son mode rapide `--in-diff` a un angle mort exactement sur ce cas : « The diff is only matched against the code under test, not the test code. So, a diff that only deletes or changes test code won't cause any mutants to run » ([in-diff.md L18-L22](https://github.com/sourcefrog/cargo-mutants/blob/6c720ae82a17/book/src/in-diff.md?plain=1#L18-L22)). Une PR qui ne fait qu'affaiblir des tests passe donc `--in-diff` : il faut une passe complète périodique.
- `#[ignore]` ne peut pas être interdit ici : les tests de bout en bout l'utilisent légitimement (`crates/claude-agent-kit/tests/e2e/*.rs`). Ce qui se surveille est l'inventaire, pas la présence de l'attribut.
- Colonnes « cliquet » et « patch » : raisonnement sur ce que chaque chiffre mesure, non vérifié par expérience.

## 3. Versions minimales des dépendances

Vocabulaire. *Résolveur* : la partie de Cargo qui choisit une version pour chaque dépendance. *Dépendance directe* : déclarée dans le `Cargo.toml` du projet ; *transitive* : tirée par une autre dépendance. *Fichier de verrouillage* (`Cargo.lock`) : la liste figée des versions choisies. *Plancher* : la plus petite version acceptée par une contrainte comme `serde = "1.0.229"`. *MSRV* : la plus ancienne version de Rust que le projet s'engage à supporter.

### 3.1 Statut dans Cargo (vérifié)

- Les deux drapeaux restent instables, donc réservés à nightly ; la doc les rattache au même ticket de suivi, [rust-lang/cargo#5657](https://github.com/rust-lang/cargo/issues/5657), ouvert depuis le 2018-06-26 et toujours ouvert (dernier commentaire le 2026-03-06, lu par l'API).
- `-Z minimal-versions` porte un avertissement : « It is not recommended to use this feature. Because it enforces minimal versions for all transitive dependencies, its usefulness is limited since not all external dependencies declare proper lower version bounds. It is intended that it will be changed in the future to only enforce minimal versions for direct dependencies. » ([doc](https://doc.rust-lang.org/cargo/reference/unstable.html#minimal-versions)).
- `-Z direct-minimal-versions` n'abaisse que les dépendances directes : « Indirect dependencies are resolved as normal so as not to be blocked on their minimal version validation. » ([doc](https://doc.rust-lang.org/cargo/reference/unstable.html#direct-minimal-versions)).
- Position exprimée dans le ticket par epage (contributeur Cargo d'après l'API GitHub) : la variante `direct` « fits perfectly for minimal version testing. However, its a pain because you need your version reqs to be the highest of your dep tree (it effectively pins to the lowest version) » ([commentaire du 2024-08-09](https://github.com/rust-lang/cargo/issues/5657#issuecomment-2278354432)).
- Le guide CI stable de Cargo ne parle pas des versions minimales (aucune occurrence de « minimal »). Il traite le cas symétrique, les dernières versions, et liste les compromis : job en « continue on failure » (échecs peu visibles), job planifié (un service CI peut désactiver les jobs planifiés d'un dépôt inactif, et les notifications n'arrivent pas toujours à la bonne personne), ou mises à jour par PR ([guide](https://doc.rust-lang.org/cargo/guide/continuous-integration.html#verifying-latest-dependencies)).

Le job actuel (`ci.yml:116-139`) utilise la variante recommandée, résolue par nightly puis testée sur stable.

### 3.2 `cargo-minimal-versions` et ce qu'il ajoute (vérifié, README au commit `f9c33ec`, v0.1.38)

- Il sépare la résolution (`cargo update -Z ...`) de la compilation, retire les dépendances de développement avant de résoudre (« dev-dependencies may raise version requirements »), et s'appuie sur cargo-hack pour traiter correctement un espace de travail à plusieurs crates ([README L83-L91](https://github.com/taiki-e/cargo-minimal-versions/blob/f9c33ec62fd0/README.md?plain=1#L83-L91)).
- `--direct` choisit la variante `direct` ([L68-L79](https://github.com/taiki-e/cargo-minimal-versions/blob/f9c33ec62fd0/README.md?plain=1#L68-L79)) ; `--ignore-private` saute les crates `publish = false` ; `--detach-path-deps` retire les champs `path` pour reproduire ce que verra un utilisateur après publication ([L50-L61](https://github.com/taiki-e/cargo-minimal-versions/blob/f9c33ec62fd0/README.md?plain=1#L50-L61)).
- Il modifie puis restaure `Cargo.toml` et `Cargo.lock` ([L48](https://github.com/taiki-e/cargo-minimal-versions/blob/f9c33ec62fd0/README.md?plain=1#L48)), et exige nightly plus cargo-hack ([L100-L101](https://github.com/taiki-e/cargo-minimal-versions/blob/f9c33ec62fd0/README.md?plain=1#L100-L101)).

Deux écarts avec le job actuel, tirés de ces lignes :

- Le job lance `cargo test`, donc résout avec les dépendances de développement. Elles peuvent remonter une version partagée et masquer un plancher trop bas d'une dépendance normale (*inférence* à partir de L87 ; non observé ici).
- La crate principale dépend de la crate de macros par `path` et `version` (`crates/claude-agent-kit/Cargo.toml:22`). C'est le cas visé par `--detach-path-deps` : il comptera à la publication, pas avant.

### 3.3 Le job qui passe au rouge sans changement du dépôt

Mécanismes, du mieux documenté au plus déduit :

1. **Échec de résolution** (vérifié). Avec `direct`, la dépendance directe est fixée à son plancher. Si une dépendance transitive exige plus, Cargo refuse. Cas réel rapporté par un mainteneur de rustls : le projet déclarait `zeroize = "^1.6.0"`, `aws-lc-rs` exigeait `^1.7`, résultat `failed to select a version for zeroize` ([commentaire](https://github.com/rust-lang/cargo/issues/5657#issuecomment-1914446399)). Réponse d'epage : c'est le comportement voulu, il faut relever son propre plancher ([commentaire](https://github.com/rust-lang/cargo/issues/5657#issuecomment-1914669002)).
2. **Pourquoi cela arrive sans toucher au dépôt** (*inférence* directe de la doc) : les transitives sont résolues « as normal », donc à la dernière version publiée le jour de l'exécution. Une nouvelle version d'une transitive qui relève sa propre exigence déclenche le cas 1 du jour au lendemain.
3. **Planchers erronés chez les autres** (vérifié, variante complète seulement) : c'est l'objet de l'avertissement de la doc, et la raison des correctifs manuels listés en 3.4.
4. **Nightly** : le job dépend du Cargo du jour (*inférence* ; nextest invoque le même risque pour son job de couverture).

Trace locale (vérifié) : le diff non commité de `Cargo.toml` relève huit planchers en même temps que l'ajout du job, par exemple `libc` de `"0.2"` à `"0.2.183"`, `futures-core` de `"0.3"` à `"0.3.31"`, `proc-macro2` de `"1.0"` à `"1.0.91"` (`Cargo.toml:18-41`). Cinq sont imposés par la résolution, `tracing` par la compilation d'une dépendance (`rmcp`), `tempfile` et `trybuild` par la compilation et les tests du projet. C'est exactement le coût décrit par epage : les contraintes doivent rejoindre la plus haute exigence de l'arbre. *Inférence* : chaque future casse de type 1 se réglera par un relèvement de plus.

### 3.4 Planification et gestion de la casse dans des projets Rust (vérifié)

| Projet | Commande | Quand | Gestion de la casse | Source |
|---|---|---|---|---|
| tokio | `cargo update -Z minimal-versions` puis `cargo hack check`, dépendances de développement retirées | PR et push | - | [ci.yml L799-L831](https://github.com/tokio-rs/tokio/blob/f3e855fa55d6/.github/workflows/ci.yml#L799-L831) |
| hyper | `cargo minimal-versions check` | PR et push | - | [CI.yml L332-L343](https://github.com/hyperium/hyper/blob/b1e7eabff468/.github/workflows/CI.yml#L332-L343) |
| rustls | `cargo minimal-versions --direct --ignore-private check` | PR, push et cron quotidien | Passage à `--direct` | [build.yml L13-L14, L326-L350](https://github.com/rustls/rustls/blob/2cd8d27ab991/.github/workflows/build.yml#L326-L350) |
| anyhow, serde (dtolnay) | `cargo generate-lockfile -Z minimal-versions` puis `cargo check --locked` | PR, push et cron quotidien | - | [anyhow L7, L70-L80](https://github.com/dtolnay/anyhow/blob/87c8571326d6/.github/workflows/ci.yml#L70-L80), [serde L115-L123](https://github.com/serde-rs/serde/blob/aa7913ad0211/.github/workflows/ci.yml#L115-L123) |
| crossbeam | `cargo minimal-versions build --workspace --all-features` | PR, push et cron quotidien | - | [ci.yml L10-L11, L147-L158](https://github.com/crossbeam-rs/crossbeam/blob/a28f40fdddb7/.github/workflows/ci.yml#L147-L158), [dependencies.sh](https://github.com/crossbeam-rs/crossbeam/blob/a28f40fdddb7/ci/dependencies.sh) |
| clap | `cargo +nightly generate-lockfile -Z minimal-versions` puis `cargo +stable check --locked` | PR et push | - | [ci.yml L180-L198](https://github.com/clap-rs/clap/blob/54605ceeee4d/.github/workflows/ci.yml#L180-L198) |
| reqwest | `cargo update -Z minimal-versions` puis `cargo check` | PR et push | `cargo update -p prefix-trie --precise 0.8.4`, avec le commentaire « fix up deps of deps, which don't properly specify minimum versions » | [ci.yml L303-L316](https://github.com/seanmonstar/reqwest/blob/6b99ec2a6a4f/.github/workflows/ci.yml#L303-L316) |
| tower | idem, puis `cargo check --locked` au MSRV | PR et push | `cargo update -p lazy_static --precise 1.5.0` | [CI.yml L42-L49](https://github.com/tower-rs/tower/blob/df06d70dbea3/.github/workflows/CI.yml#L42-L49) |
| axum | idem, puis `cargo test --locked` au MSRV | PR et push | Trois `--precise` (crc32fast, version_check, lazy_static) | [CI.yml L130-L145](https://github.com/tokio-rs/axum/blob/2449f1600c66/.github/workflows/CI.yml#L130-L145) |
| rand | `cargo generate-lockfile -Z minimal-versions` puis tests | PR, push et cron hebdomadaire | - | [test.yml L14-L15, L101-L104](https://github.com/rust-random/rand/blob/c03e547d051f/.github/workflows/test.yml#L101-L104) |
| jonhoo/rust-ci-conf | `cargo +nightly update -Zminimal-versions` puis `cargo test --locked` | PR et push ; le nightly et `cargo update` vont dans un fichier « rolling » à cron quotidien | Dépendance factice pour relever un plancher transitif, ou bascule vers `direct` | [test.yml L46-L88](https://github.com/jonhoo/rust-ci-conf/blob/0de345fbe62c/.github/workflows/test.yml#L46-L88), [scheduled.yml L1-L11](https://github.com/jonhoo/rust-ci-conf/blob/0de345fbe62c/.github/workflows/scheduled.yml#L1-L11) |
| rust-bitcoin | Deux fichiers de verrouillage commités, matrice `dep: [minimal, recent]` | PR et push | Sans objet : résolution figée | [rust.yml L25, L46](https://github.com/rust-bitcoin/rust-bitcoin/blob/271f662a6890/.github/workflows/rust.yml#L25-L46), [CONTRIBUTING L263-L270](https://github.com/rust-bitcoin/rust-bitcoin/blob/271f662a6890/CONTRIBUTING.md?plain=1#L263-L270) |

Lecture :

- Les douze lignes lancent le contrôle à chaque PR. Quatre y ajoutent un cron (rustls, dtolnay, crossbeam, rand). Aucune ne le limite aux changements de manifeste par un filtre de chemins.
- La plupart font `check` (compiler sans exécuter), pas `test`. Seuls axum, rand, jonhoo et rust-bitcoin exécutent les tests.
- La variante complète reste majoritaire malgré l'avertissement de la doc ; ceux qui cassent corrigent à la main dans le fichier CI.
- Le fichier `ci.yml` du projet reprend la forme du gabarit jonhoo, avec `direct` à la place.

### 3.5 Variante déterministe (vérifié pour le principe)

- **Fichier de verrouillage minimal commité.** rust-bitcoin tient `Cargo-minimal.lock` et `Cargo-recent.lock` dans le dépôt, les régénère par `just update-lock-files` quand un manifeste change, et la CI les copie ou les sélectionne (liens ci-dessus). Le job ne dépend alors plus ni de nightly ni de l'état de crates.io le jour de l'exécution.
- **Liste de `cargo update -p X --precise V`.** Décrite dès 2019 dans le ticket de suivi ([commentaire](https://github.com/rust-lang/cargo/issues/5657#issuecomment-466135732)) ; c'est ce que font tower, axum et reqwest pour quelques crates.

*Inférence, non testée* : un verrou minimal commité peut vieillir en silence. `--locked` échoue si un plancher est relevé (le verrou ne satisfait plus le manifeste), mais passe si un plancher est abaissé : le nouveau plancher n'est alors plus testé. Il faut un contrôle de fraîcheur (régénérer et comparer) sur changement de manifeste ou par cron.

### 3.6 Comparaison avec le dépôt Python (vérifié)

- Un job dédié, `test-mcp-v1-floor`, installe le projet puis force `mcp==1.23.0`, vérifie la version installée, et lance mypy et la suite de tests (`test.yml:44-70`). Il tourne à chaque PR et à chaque push sur `main` (`test.yml:3-7`). Le job principal affirme à l'inverse qu'il teste bien la dernière version majeure (`test.yml:29-32`).
- Un seul plancher sur cinq est testé : `mcp>=1.23.0,<3.0.0` (`pyproject.toml:31`). Ceux de `anyio`, `sniffio`, `typing_extensions` et `jsonschema` (`pyproject.toml:28-32`) ne le sont pas.
- C'est déterministe pour la version nommée, puisqu'elle est écrite en dur ; les autres paquets prennent la dernière version du jour.
- Le dépôt n'utilise pas la résolution basse automatique qu'offre uv, dont la doc recommande pourtant « to separately run tests with `--resolution lowest` or `--resolution lowest-direct` in continuous integration » pour une bibliothèque ([doc uv L319-L374](https://github.com/astral-sh/uv/blob/d5c3f1744174/docs/concepts/resolution.md?plain=1#L319-L374)).

Lecture : le choix amont est un plancher ciblé, là où une plage large est annoncée et où deux versions majeures coexistent, pas un contrôle général.

## 4. Garder à jour des actions épinglées par SHA sans robot contributeur

Vocabulaire. *Épingler par SHA* : écrire `uses: owner/action@<40 caractères>` au lieu d'une étiquette déplaçable comme `@v2`. *Auteur* et *committer* : Git enregistre deux identités par commit, celle qui a écrit le changement et celle qui l'a appliqué. *Trailer* : ligne `Clé: valeur` en fin de message, par exemple `Co-authored-by:`. *Cooldown* : délai imposé avant d'adopter une version fraîchement publiée.

### 4.1 Auteur et committer selon la méthode de fusion (vérifié)

| Méthode | Ce qui arrive sur la branche par défaut | Auteur | Committer | Sources |
|---|---|---|---|---|
| Commit de fusion | Les commits de la PR, inchangés, plus un commit de fusion | Commits de la PR : le robot. Commit de fusion : la personne qui fusionne | Celui d'origine pour les commits de la PR | Doc : le sélecteur d'e-mail fixe l'auteur du commit de fusion ([doc](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/incorporating-changes-from-a-pull-request/merging-a-pull-request)). Observé chez clap : fusion [`89b1243`](https://github.com/clap-rs/clap/commit/89b12436f) (auteur humain, 2 parents) et, juste avant dans l'historique, [`f9172e6`](https://github.com/clap-rs/clap/commit/f9172e650) (auteur `renovate[bot]`, même sujet que la branche fusionnée ; lien de parenté déduit, non relu) |
| Squash (tous les commits écrasés en un seul) | Un commit unique | **Celui qui a ouvert la PR**, donc le robot ; chaque auteur de commit de la PR est ajouté en `Co-authored-by` | GitHub | « whoever opened the pull request became the sole author of the squash commit. Now, we will automatically credit every commit author in the pull request as a co-author » ([annonce du 2019-12-19](https://github.blog/changelog/2019-12-19-improved-attribution-when-squashing-commits/)). Observé chez tokio : [`66e1387`](https://github.com/tokio-rs/tokio/commit/66e13876a7f0251309d61aef9e9d2821add6f356), auteur `dependabot[bot]`, committer `GitHub`, deux trailers `Co-authored-by` |
| Rebase (commits rejoués un par un) | Des commits recréés | Inchangé : le robot | Réécrit ; commits non signés | « Always updates the committer information and creates new commit SHAs » ([doc](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/about-merge-methods-on-github)). Observé chez rustls : [`dfa0959`](https://github.com/rustls/rustls/commit/dfa095933651f8800c4f4976a7a9d5ef6534ec9e), auteur `dependabot[bot]`, committer humain, non signé |

Conclusion vérifiée : dans les trois cas, le robot reste auteur d'au moins un commit sur la branche par défaut. Aucune méthode de fusion ne l'efface.

### 4.2 Ce qui fait apparaître un compte parmi les contributeurs (vérifié)

- Le graphe des contributeurs compte les commits de la branche par défaut dont l'e-mail d'auteur est rattaché à un compte ; les commits de fusion et les commits vides ne comptent pas ([doc](https://docs.github.com/en/repositories/viewing-activity-and-data-for-your-repository/viewing-a-projects-contributors)).
- Un co-auteur compte aussi, si l'e-mail du trailer est rattaché à son compte ([doc](https://docs.github.com/en/pull-requests/committing-changes-to-your-project/creating-and-editing-commits/creating-a-commit-with-multiple-authors)).
- Les robots sont listés comme les humains : l'API des contributeurs renvoie `dependabot[bot]` pour [tokio](https://api.github.com/repos/tokio-rs/tokio/contributors?per_page=100) (6 contributions), et `renovate[bot]` (156), `bors[bot]` (327), `dependabot[bot]` (14) pour [clap](https://api.github.com/repos/clap-rs/clap/contributors?per_page=100).

Règles pratiques qui en découlent (*inférence*, cohérente avec 4.1) :

- Tout commit qui atteint `main` doit avoir l'e-mail du propriétaire comme auteur.
- Aucun message ne doit porter de `Co-authored-by` vers un compte de robot.
- En squash, la PR doit être ouverte par le propriétaire et ne contenir que ses commits : GitHub ajoute de lui-même un trailer par auteur présent dans la PR.

### 4.3 Les options

**A. Dependabot.**

- Il met à jour les références par SHA et le commentaire de version placé sur la même ligne ; si le commit épinglé ne correspond à aucune étiquette, il passe au dernier commit ([doc, section Dependabot](https://docs.github.com/en/actions/reference/security/secure-use)). Ce dernier cas concerne `dtolnay/rust-toolchain@683b588... # master` (`ci.yml:31`).
- Cooldown : 3 jours par défaut même sans configuration, réglable par `default-days` ; pour GitHub Actions, les variantes par niveau semver ne sont pas supportées ([doc](https://docs.github.com/en/code-security/dependabot/working-with-dependabot/dependabot-options-reference#cooldown)). Regroupement en une seule PR par `groups` ([doc](https://docs.github.com/en/code-security/dependabot/working-with-dependabot/dependabot-options-reference#groups)).
- Ses exécutions ne consomment pas les minutes Actions incluses ([doc](https://docs.github.com/en/code-security/dependabot/working-with-dependabot/about-dependabot-on-github-actions-runners)). La CI déclenchée par ses PR, si.
- Les alertes de sécurité Dependabot ne couvrent pas les actions épinglées par SHA ([doc](https://docs.github.com/en/actions/reference/security/secure-use)).
- La référence des options ne contient aucune clé d'auteur ou de committer (recherche des deux mots dans la page : aucune occurrence).

Verdict : fusionner une PR Dependabot viole la règle (4.1). Il reste utilisable comme simple signal, le changement étant refait localement sous l'identité du propriétaire ; le robot apparaît alors comme auteur de PR, pas comme contributeur. Fermeture automatique de sa PR une fois la mise à jour faite autrement : non vérifié.

**B. Renovate.**

- Application hébergée : l'auteur est `renovate[bot]` (observé chez clap, 4.1). Exclue.
- Auto-hébergé avec un jeton personnel (PAT) : sans réglage, Renovate déduit l'auteur Git du compte lié au jeton ; `gitAuthor` permet de le fixer, « although we do not recommend this » ([doc L2071-L2088](https://github.com/renovatebot/renovate/blob/9ac381180859/docs/usage/configuration-options.md?plain=1#L2071-L2088)). Le commit et la PR sont alors ceux du propriétaire.
- Risque documenté au même endroit : si un humain partage l'e-mail de Renovate et pousse sur une de ses branches, Renovate peut « force push over the changes ».
- Le jeton doit porter les droits `repo` et `workflow` ([doc L25-L26](https://github.com/renovatebot/renovate/blob/9ac381180859/lib/modules/platform/github/readme.md?plain=1#L25-L26)) : un secret large et durable à stocker.
- Cooldown par `minimumReleaseAge`, épinglage par `pinDigests` ([doc L3005-L3013, L4272-L4274](https://github.com/renovatebot/renovate/blob/9ac381180859/docs/usage/configuration-options.md?plain=1#L3005-L3013)). Avec `platformCommit` (mode application), c'est GitHub qui fixe l'auteur et `gitAuthor` est ignoré ([L4312-L4321](https://github.com/renovatebot/renovate/blob/9ac381180859/docs/usage/configuration-options.md?plain=1#L4312-L4321)).

**C. Outils en ligne de commande, lancés localement.** Le commit est écrit par qui lance l'outil.

| Outil | Dernière version (API GitHub) | Ce qu'il fait | Source |
|---|---|---|---|
| pinact | v5.0.2, 2026-10-25 | `pinact run --update` met à jour ; `--min-age` impose un âge minimal ; `--check` échoue sans rien écrire ; vérifie que le commentaire de version dit vrai | [README L43-L54, L73-L79](https://github.com/suzuki-shunsuke/pinact/blob/bd926f71d69c/README.md?plain=1#L43-L79), [update.md](https://github.com/suzuki-shunsuke/pinact/blob/bd926f71d69c/docs/update.md?plain=1#L7-L53) |
| ratchet | v0.12.0, 2026-07-18 | `ratchet update` (dans la contrainte), `ratchet upgrade` (dernière version), `-bake-delay 24h` (cooldown, désactivé par défaut) | [README L132-L185](https://github.com/sethvargo/ratchet/blob/a7ead07a8997/README.md?plain=1#L132-L185) |
| zizmor | v1.32.0, 2026-11-02 | Audit, pas mise à jour : `unpinned-uses` (exige le SHA pour toutes les actions depuis v1.20.0, `actions/*` comprises), `ref-version-mismatch` (commentaire qui ne correspond pas au SHA), `impostor-commit` (SHA venant d'un fork) | [audits.md L2237, L1489, L1079](https://github.com/zizmorcore/zizmor/blob/cf6865501ab4/docs/audits.md?plain=1#L2237) |
| actions-up, frizbee | v2.0.0 du 2026-10-21 ; v0.1.11 du 2026-08-14 | Existent et publient ; fonctions non lues | API des releases |

Ces outils n'avertissent de rien par eux-mêmes. Un job planifié peut lancer la mise à jour puis `git diff --exit-code` et échouer s'il y a un écart, sans rien commiter (*inférence* : montage non testé).

**D. Règle GitHub « Require actions to be pinned to a full-length commit SHA ».** Réglage de dépôt ou d'organisation : « all actions must be pinned to a full-length commit SHA to be used. This includes actions from your organization and actions authored by GitHub » ([doc](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/enabling-features-for-your-repository/managing-github-actions-settings-for-a-repository)). Elle impose l'épinglage, pas la fraîcheur, et bloquerait aujourd'hui `actions/checkout@v7` et `actions/upload-artifact@v7` (`ci.yml:29`, `ci.yml:176`).

**E. Releases immuables.** L'étiquette et les fichiers d'une release immuable ne peuvent plus être modifiés ([doc](https://docs.github.com/en/code-security/supply-chain-security/understanding-your-software-supply-chain/immutable-releases)). Deux limites :

- Cela protège une étiquette complète (`v7.0.1`), pas une étiquette flottante comme `v7`, déplacée à chaque version.
- C'est au choix de l'éditeur. Champ `immutable` de l'API pour la dernière release, lu ce jour : `true` pour `taiki-e/install-action` v2.90.5 ; `false` pour `actions/checkout` v7.0.1, `actions/upload-artifact` v7.0.2, `Swatinem/rust-cache` v2.9.3 et `EmbarkStudios/cargo-deny-action` v2.1.1.

Laisser `actions/*` sur étiquette est donc un choix de confiance, que la doc formule ainsi : « Pin actions to a tag only if you trust the creator » ([doc](https://docs.github.com/en/actions/reference/security/secure-use)).

**F. Fichier de verrouillage des workflows (`gh actions-lock`).** Trouvé en cours de recherche, hors de la liste de départ.

- Disponible pour tous depuis le 2026-10-28, offre Free comprise ([annonce](https://github.blog/changelog/2026-10-28-github-actions-workflow-lockfiles-are-generally-available/)).
- La doc de sécurité le place en tête : « This is the strongest protection available, and supersedes SHA pinning for repositories that adopt it » ; l'épinglage par SHA y devient le repli pour les dépôts « that cannot adopt a lockfile » ([doc](https://docs.github.com/en/actions/reference/security/secure-use)).
- Principe : `.github/workflows/actions.lock`, généré par une extension de la CLI GitHub, fixe un commit pour chaque action, y compris les actions appelées par d'autres actions (ce que l'épinglage dans le fichier de workflow ne couvre pas), et enregistre l'identité numérique du dépôt source. Les workflows gardent des étiquettes lisibles ([concept](https://docs.github.com/en/actions/concepts/security/workflow-dependency-lockfiles)).
- Usage : `gh actions-lock` génère ou répare, `--relock` avance les références déplacées, `--verify` contrôle en CI sans écrire ([README L51-L59](https://github.com/github/gh-actions-lock/blob/90ae563599cf/README.md?plain=1#L51-L59), [guide](https://docs.github.com/en/actions/how-tos/secure-your-work/lock-workflow-dependencies)). Lancé localement, le commit est celui du propriétaire.
- Un verrou satisfait la règle D sans SHA dans le workflow ([doc](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/enabling-features-for-your-repository/managing-github-actions-settings-for-a-repository)).
- Limites : workflows réutilisables d'autres dépôts et images de conteneur non verrouillés ; actions locales `./` non suivies ([README L70-L76](https://github.com/github/gh-actions-lock/blob/90ae563599cf/README.md?plain=1#L70-L76)). Outil jeune : v0.4.1 du 2026-11-02, format de verrou `v0.0.3`.
- pinact s'y est adapté : il laisse au verrou ce que le verrou couvre ([README L29-L41](https://github.com/suzuki-shunsuke/pinact/blob/bd926f71d69c/README.md?plain=1#L29-L41)).

Le verrou règle l'intégrité, pas la fraîcheur : il faut toujours que quelqu'un lance `--relock` ou change les références.

### 4.4 Ce que font des projets Rust (vérifié)

| Projet | Références | Mise à jour | Source |
|---|---|---|---|
| tokio | Étiquettes (0 SHA dans `ci.yml`) | Dependabot hebdomadaire | [dependabot.yml](https://github.com/tokio-rs/tokio/blob/f3e855fa55d6/.github/dependabot.yml) |
| rustls, hyper, anyhow | Étiquettes (0 SHA) | Non relevé | Fichiers CI cités en 3.4 |
| clap | 24 références par SHA | Renovate hébergé, mensuel, `helpers:pinGitHubActionDigests`, `minimumReleaseAge: "3 days"` | [renovate.json5 L1-L6](https://github.com/clap-rs/clap/blob/54605ceeee4d/.github/renovate.json5#L1-L6) |
| crossbeam, cargo-llvm-cov | SHA (21 et 11 références dans leurs `ci.yml`) | Dependabot mensuel, cooldown de 14 jours | [crossbeam L12-L17](https://github.com/crossbeam-rs/crossbeam/blob/a28f40fdddb7/.github/dependabot.yml#L12-L17), [cargo-llvm-cov L16-L21](https://github.com/taiki-e/cargo-llvm-cov/blob/dc927946e73a/.github/dependabot.yml#L16-L21) |
| jonhoo/rust-ci-conf | SHA | Dependabot mensuel, mises à jour mineures et correctives groupées | [dependabot.yml L3-L12](https://github.com/jonhoo/rust-ci-conf/blob/0de345fbe62c/.github/dependabot.yml#L3-L12) |
| nextest | SHA | Renovate, configuration partagée | [renovate.json](https://github.com/nextest-rs/nextest/blob/724cd7f904d8/.github/renovate.json) |

Lecture : tous les projets qui épinglent par SHA s'appuient sur un robot, et ce robot figure parmi leurs contributeurs. Aucun exemple de mise à jour manuelle par outil local n'a été trouvé. La contrainte du propriétaire sort de la pratique courante : les projets de référence n'apportent ici qu'un réglage de cadence (mensuel, cooldown de 3 à 14 jours).

### 4.5 Ce que fait le dépôt Python (vérifié)

- **Épinglage partiel.** Les actions tierces sont épinglées par SHA avec un commentaire `# vX.Y.Z (sha-pinned)` (`test.yml:39`, `test.yml:75`, `pypi-quota-check.yml:41`, `release-alert.yml:75`, `slack-issue-notification.yml:65`). Les `actions/*` restent sur étiquette (`actions/checkout@v4`, `actions/setup-python@v5`, par exemple `test.yml:17-20`), sauf `actions/github-script`, épinglée dans l'action locale (`.github/actions/setup-claude-auth/action.yml:15`). `anthropics/claude-code-action@v1` est sur étiquette (`claude-code-review.yml:34`).
- **Aucun automate de mise à jour.** Pas de `dependabot.yml`, pas de configuration Renovate, aucune mention de pinact, ratchet ou zizmor dans `.github/`, `scripts/`, `CLAUDE.md`, `RELEASING.md`, `README.md`.
- **Résultat mesurable.** L'épinglage date du commit `3c08a3a` (2026-05-06, écrit par un humain) et n'a pas bougé depuis : `codecov-action` v4.6.0 alors que la dernière étiquette est v7.1.1, `slack-github-action` v2.1.1 contre v4.0.0, `actions/checkout@v4` contre v7.0.1 (étiquettes lues par `git ls-remote` ce jour). Sans rappel, l'épinglage vieillit.
- **Outils de construction.** Versions et empreintes figées par `uv pip compile --generate-hashes` (`scripts/requirements-build.txt:1-2`, `scripts/requirements-recompress.txt:1-8`), installées avec `--require-hashes` (`build-wheel-check.yml:48-49`), mises à jour à la main (deux commits humains dans l'historique de ces fichiers).
- **Outils de développement.** Bornes basses ouvertes (`pytest>=7.0.0`, `ruff>=0.1.0`, `mypy>=1.0.0`, `pyproject.toml:36-42`) : toujours la dernière version, sans déterminisme.
- **CLI embarqué.** Un commit automatique `chore: bump bundled CLI version to X` arrive de l'extérieur du dépôt (auteur `GitHub Actions <actions@github.com>`), et déclenche la publication (`auto-release.yml:12-15`, `RELEASING.md:32-43`).
- **Les robots y sont des auteurs ordinaires.** Sur les 300 derniers commits : 109 de `GitHub Actions`, 86 de `github-actions[bot]`, 52 de `claude[bot]`. La publication configure cette identité (`build-and-publish.yml:271-272`). Ce modèle n'est pas transposable à la règle du propriétaire.

Ce que vérifie le script de durcissement (`.github/scripts/check_workflow_hardening.py`, lancé à chaque PR et push sur `main` par `workflow-hardening.yml:5-8`) :

- Périmètre : uniquement les jobs qui appellent Claude, reconnus par l'action Claude Code ou par la mention de `ANTHROPIC_FEDERATION_RULE_ID` (`:99-108`).
- Ces jobs tournent sur le runner à pare-feu sortant `ubuntu-24.04-firewall`, avec une table d'exceptions motivées (`:14`, `:30-32`, `:233-277`).
- Chaque étape Claude passe `--permission-mode auto`, sans `--dangerously-skip-permissions`, sans `defaultMode` dans les réglages, sur un modèle au moins 4.6 (`:20-22`, `:111-191`, `:284-289`).
- La liste blanche réseau existe, est en `mode: enforce`, non vide, sans joker `*` (`:301-333` ; fichier `.github/egress-firewall.yaml:4-39`).
- **Il ne vérifie ni l'épinglage par SHA ni la fraîcheur des versions** : aucune logique sur le suffixe `@...` des `uses:`. Les trois protections sont décrites dans `CLAUDE.md:29-80`.

### 4.6 État des références du projet (vérifié ce jour par `git ls-remote`)

- Les trois SHA commentés par une version pointent bien l'étiquette annoncée : `Swatinem/rust-cache` v2.9.3, `EmbarkStudios/cargo-deny-action` v2.1.1, `taiki-e/install-action` v2.90.3 (`ci.yml:36`, `ci.yml:94`, `ci.yml:158`).
- `taiki-e/install-action` en est déjà à v2.90.5, publiée ce jour. Ce dépôt publie très souvent (d'autres projets sont sur v2.85.13, v2.87.21, v2.87.29) : suivre chaque version n'a pas de sens, une cadence mensuelle avec cooldown suffit.
- `dtolnay/rust-toolchain@683b588... # master` : ce dépôt n'a pas d'étiquettes de version utilisées ici ; la branche `master` est déjà à `df2a88e`. nextest épingle le même SHA avec le même commentaire.

## 5. Adéquation pour un ingénieur seul qui travaille avec des agents

Repère de coût : sur dépôt privé, la minute coûte 0,006 USD sous Linux, 0,010 sous Windows, 0,062 sous macOS, soit dix fois Linux ([tarifs](https://docs.github.com/en/billing/reference/actions-runner-pricing)), pour 12 USD inclus par mois en Free et 18 en Pro ([facturation](https://docs.github.com/en/billing/concepts/product-billing/github-actions)). Les trois jobs discutés ici tournent sous Linux : leur coût est faible devant celui de la matrice à trois systèmes (`ci.yml:20-27`). Les durées réelles n'ont pas été mesurées.

### 5.1 Couverture

| Option | Attention humaine récurrente | Risque de contrôle qui pourrit en silence | Minutes CI |
|---|---|---|---|
| Rapport seul (état actuel) | Aucune imposée ; n'a de valeur que si le tableau est lu | Élevé : c'est l'état du dépôt Python (2.1) | Un job Linux, déjà payé |
| Seuil fixe | Presque nulle | Moyen : le seuil décroche de la réalité quand la couverture monte | Aucune de plus |
| Cliquet local avec tolérance | Faible : relever le plancher, arbitrer les baisses légitimes | Faible si le plancher est comparé à la branche de base | Aucune de plus |
| Couverture du patch locale | Moyenne : exceptions pour le code non testable | Faible | Quelques secondes, plus un outil Python à installer et à épingler |
| Codecov ou règle GitHub native | Faible | Faible, mais dépend d'un tiers ou d'une offre payante (2.3) | Aucune de plus |
| Test de mutation, complet et périodique | Moyenne : trier les mutants survivants | Faible | Élevé : une compilation et une suite de tests par mutant ; à planifier, pas à chaque PR |

Ce que les faits établissent :

- Le seuil fixe est dominé par le cliquet : même coût, plus de signal, et c'est ce que font les projets qui bloquent (rustls à 0,05 %, jonhoo à 1 %).
- Aucun chiffre de couverture ne répond au risque propre aux agents, le test affaibli. Seul le test de mutation y répond, et son mode rapide ne voit pas les changements limités aux tests.
- Une barrière dans le dépôt peut être abaissée par la PR qu'elle juge.

Ce qui trancherait : l'offre GitHub du compte (sans Pro, rien n'est « obligatoire » sur un dépôt privé) ; et si le propriétaire relit lui-même chaque diff de tests. S'il ne le fait pas, le chiffre de couverture ne suffit pas, quel que soit le mécanisme.

### 5.2 Versions minimales

| Option | Attention humaine récurrente | Risque de pourrissement silencieux | Minutes CI |
|---|---|---|---|
| État actuel : `direct`, résolu à chaque PR et push, suite complète | Moyenne et imprévisible : une casse externe interrompt une PR sans rapport | Nul | Un job Linux complet par PR et par push |
| Même commande, sur cron et sur changement de manifeste | Faible, mais l'échec arrive hors de toute PR | Moyen : notifications mal routées, cron désactivable (guide Cargo, 3.1) | Faible |
| Verrou minimal commité | Faible : régénérer quand un manifeste change (un agent peut le faire, avec nightly en local) | Moyen sans contrôle de fraîcheur (3.5), faible avec | Comme l'état actuel, sans installation de nightly |
| `cargo minimal-versions --direct check` | Comme l'état actuel | Nul | Plus faible : compile sans exécuter |

Ce que les faits établissent :

- `direct` plutôt que la variante complète : la doc de Cargo et epage vont dans le même sens. Le job actuel est déjà de ce côté.
- Le rouge sans changement est structurel pour toute résolution faite le jour de l'exécution. Seul le verrou commité le supprime.
- Sur la cadence, pas de consensus : tous les projets lus le lancent à chaque PR, quatre lignes sur douze y ajoutent un cron.

Ce qui trancherait : la date de publication. Le contrôle protège les utilisateurs d'une bibliothèque publiée, et la crate ne l'est pas encore : d'ici là, le bénéfice est surtout de garder des planchers justes au fil de l'eau plutôt que de les découvrir tous à la publication. L'autre critère est la tolérance du propriétaire aux interruptions sans rapport avec le travail en cours.

### 5.3 Actions épinglées

| Option | Attention humaine récurrente | Risque de pourrissement silencieux | Minutes CI | Compatible avec « un seul contributeur » |
|---|---|---|---|---|
| Dependabot ou Renovate hébergé, PR fusionnées | Faible | Nul | CI sur chaque PR | **Non** (4.1) |
| Dependabot comme signal, changement refait localement | Une action par PR (une par mois si groupée) | Nul : la PR reste ouverte | CI sur des PR jamais fusionnées | Oui pour les commits ; le robot reste visible comme auteur de PR (cas limite à trancher par le propriétaire) |
| Renovate auto-hébergé avec le jeton du propriétaire | Relire et fusionner | Moyen : un jeton expiré arrête tout sans bruit (*inférence*) | Une exécution planifiée, plus la CI des PR | Oui, avec le risque d'écrasement documenté (4.3 B) |
| Outil local (pinact, ratchet) lancé à la demande | Y penser | **Élevé** sans rappel : voir le dépôt Python (4.5) | Aucune | Oui |
| Outil local, plus un job planifié qui échoue en cas d'écart | Un job rouge par mois, puis une commande | Faible | Quelques minutes par mois | Oui |
| Verrou `gh actions-lock`, plus `--verify` en CI | Comme la ligne précédente | Faible pour l'intégrité ; la fraîcheur demande toujours un rappel | Quelques secondes par exécution | Oui |

Ce que les faits établissent :

- Tout robot qui écrit le commit est exclu, quelle que soit la méthode de fusion. Les options restantes écrivent toutes sous l'identité du propriétaire.
- Sans rappel automatique, l'épinglage vieillit : le dépôt Python a trois versions majeures de retard six mois après son épinglage.
- La doc de GitHub place le verrou au-dessus de l'épinglage par SHA, et il couvre les actions appelées par d'autres actions.
- Un agent ne doit pas écrire un SHA de mémoire : un outil qui le résout, et un audit qui confronte SHA et commentaire (pinact, zizmor), ferment ce risque.

Ce qui trancherait : entre SHA dans les workflows et verrou natif, la tolérance à une fonction généralisée il y a moins de trois semaines et à un outil en v0.4 ; et la lecture stricte ou non de « visible » pour un robot auteur de PR.

## 6. Vérification et limites

**Contrôles effectués :**

- Dépôt Python lu directement : les 13 workflows listés, `test.yml`, `lint.yml`, `workflow-hardening.yml`, `auto-release.yml` et le script de durcissement en entier, `egress-firewall.yaml`, `CLAUDE.md`, `RELEASING.md`, `pyproject.toml` ; les autres workflows par recherche ciblée (`uses:`, identité Git, couverture). Absence de configuration Codecov, Dependabot et Renovate constatée par recherche de fichiers.
- Projet : `ci.yml` de l'arbre de travail lu en entier ; `Cargo.toml` et son diff ; `xtask/src/parity/mod.rs` par extraits.
- Chaque fichier CI externe a été téléchargé à la tête de sa branche par défaut, le SHA de tête relevé dans la foulée par `git ls-remote`. Les numéros de ligne viennent de ces copies.
- Documentation GitHub lue par son point d'accès Markdown (`docs.github.com/api/article/body`), ce qui donne le texte mais pas les bandeaux de disponibilité ; ceux des branches protégées et des rulesets ont été relus dans le HTML.
- Auteur et committer par méthode de fusion : annonce GitHub pour le squash, doc pour le rebase et la fusion, plus un commit réel par méthode, lu par l'API.
- Correspondance SHA et étiquette vérifiée pour les références du projet et du dépôt Python.

**Découverte hors périmètre initial :** le fichier de verrouillage des workflows GitHub Actions (4.3 F), absent de la liste d'options de départ et postérieur à l'épinglage du dépôt Python.

**Non vérifié :**

- Disponibilité de GitHub Code Quality et de sa règle de couverture pour un compte personnel ; période du quota de 250 envois Codecov.
- Si l'envoi Codecov du dépôt Python fonctionne, et si une règle de protection y exige un statut.
- Seuil par crate avec cargo-llvm-cov par appels séparés de `report`.
- Tous les montages proposés mais non exécutés : cliquet local, contrôle de fraîcheur d'un verrou minimal, job planifié de détection d'écart, comportement de `gh actions-lock --verify` face à une référence simplement en retard.
- Fermeture automatique d'une PR Dependabot quand la mise à jour est faite à la main ; expiration d'un jeton comme cause d'arrêt silencieux de Renovate.
- Désactivation des workflows planifiés sur dépôt privé inactif (seul le guide Cargo a été lu sur ce point, pas la doc GitHub).
- Durées et coûts réels des jobs ; l'offre GitHub du compte.
- actions-up et frizbee : existence et date de release seulement. `patchcov` (v0.4.1) : README survolé, non évalué.
- Pour tokio, hyper et les autres projets « sans couverture » : seul le fichier CI principal a été fouillé, pas les autres workflows.
- `.github/workflows/e2e.yml` du projet n'a pas été lu.

**Confiance :** élevée sur les faits des sections 2.1, 2.2, 3.1, 3.4, 4.1, 4.2 et 4.5, tous relus à la source. Moyenne sur la section 5, qui croise ces faits avec des coûts non mesurés. Les colonnes « cliquet » et « patch » du tableau 2.5 sont un raisonnement, pas une observation.
