# Parité avec le SDK Python d'Anthropic : mesure fonctionnelle et comportementale

> Date : 2026-11-16. Base : ce dépôt au commit `c15579f` (branche `main`) ; `claude-agent-sdk-python` au commit `4720d36` (`v0.2.182` plus un commit), qui est aussi le commit du sous-module `upstream/`.
> Méthode : neuf sous-agents en lecture seule, un par sous-système, chacun avec la consigne de comparer le code au code et d'ignorer les déclarations de parité du dépôt (`docs/migration.md`, `parity/*.toml`, README). Chaque rapport a ensuite été recontrôlé par l'agent principal, qui a relu les lignes citées des deux côtés. Mesures mécaniques faites à part : `cargo xtask parity-check`, `cargo xtask parity-report`, un recomptage indépendant des exports et des tests Python, et la suite de tests hors ligne. Aucun test sur le vrai CLI n'a été lancé (appels payants). Ce qui n'a pas pu être confirmé est marqué « non vérifié » et repris en section 9.

## 1. Verdict

**La parité fonctionnelle est quasi complète. La parité comportementale est forte sur le chemin nominal, et les écarts se concentrent sur les cas limites, les textes d'erreur et quelques choix de conception.**

| Mesure | Résultat |
|---|---|
| Noms publics Python (`__all__`) ayant un équivalent Rust | 159 sur 159 |
| Capacités Python présentes en Rust (éléments comparés un par un) | environ 99,5 % (4 absences, toutes mineures ou voulues) |
| Éléments au comportement identique ou équivalent | environ 90 % |
| Éléments au comportement observable différent | environ 87, dont une minorité pose un vrai risque (section 4.4) |
| Tests unitaires Python couverts par un test Rust | 933 sur 1170 (79,7 %), soit 92 % des tests qui concernent le produit |
| Scénarios de bout en bout Python ayant un équivalent Rust | 48 sur 48 |
| Suite Rust hors ligne | 808 tests passent, 0 échec, 54 ignorés (ceux qui appellent le vrai CLI) |

Trois lectures à retenir :

1. **Rien d'important ne manque.** Options, messages, protocole de contrôle, hooks, permissions, serveurs MCP en processus, sessions, session store, reprise, miroir de transcript : tout est porté.
2. **Un écart avec Python n'est pas toujours un défaut.** Une partie des écarts rend Rust plus robuste, et une autre le rend plus fidèle au vrai CLI que Python ne l'est (section 4.2).
3. **Cinq écarts méritent une décision** (section 8) : la validation des arguments d'outil désactivée par défaut, la hiérarchie d'erreurs aplatie, un échec d'écriture des identifiants ignoré à la reprise, la perte des champs inconnus de `modelUsage`, et quatre assertions de test plus faibles que leur original.

Vocabulaire. *Parité fonctionnelle* : la même capacité existe. *Parité comportementale* : elle se comporte de la même façon (valeurs par défaut, format des messages échangés avec le CLI, erreurs, ordre, délais). *CLI* : le programme `claude` que le SDK lance et pilote. *Protocole de contrôle* : les messages JSON que le SDK et le CLI s'échangent pour initialiser une session, demander une permission ou appeler un hook. *Hook* : fonction de l'application que le CLI appelle à un moment précis (avant un outil, par exemple). *Session store* : stockage externe des transcripts de session, fourni par l'application.

## 2. Comment lire les chiffres

Chaque sous-agent a classé chaque élément de son périmètre dans une seule catégorie :

- **IDENTIQUE** : même comportement observable (une traduction idiomatique compte).
- **ADAPTÉ** : forme volontairement différente, même capacité (un `enum` Rust à la place de plusieurs classes, `Result` à la place d'une exception).
- **DIVERGENT** : le comportement observable diffère.
- **MANQUANT** : Python l'a, Rust non.
- **EN PLUS** : Rust seulement.

Un élément est un nom exporté, un champ d'option, un drapeau de ligne de commande, un type de message, une règle d'algorithme ou une condition d'erreur. Le grain varie d'un sous-système à l'autre, et plusieurs agents ont donné des comptes approchés : les pourcentages sont des ordres de grandeur, pas des mesures au point près.

## 3. Parité par sous-système

| Sous-système | Identique | Adapté | Divergent | Manquant | En plus | Capacité présente | Comportement identique ou équivalent |
|---|---|---|---|---|---|---|---|
| Surface publique, options, configuration, erreurs | 262 | 26 | 13 | 2 | 29 | 99,3 % | 95 % |
| Messages, blocs de contenu, parseur, types de hooks et de permissions | env. 175 | 13 | 12 | 0 | 4 | 100 % | 94 % |
| Transport (recherche, lancement et arrêt du CLI) | env. 84 | 7 | 18 | 1 | 8 | 99,1 % | 83 % |
| Protocole de contrôle, `query`, client interactif | 43 | 10 | 11 | 1 | 1 | 98,5 % | 82 % |
| Serveurs MCP en processus et outils | 30 | 9 | 11 | 1 | 3 | 98,0 % | 76 % |
| Lecture, liste et modification des sessions | 108 | 8 | 13 | 0 | 1 | 100 % | 90 % |
| Session store, reprise, miroir, import, conformité | 90 | 17 | 12 | 0 | 3 | 100 % | 90 % |
| **Total brut** | **792** | **90** | **90** | **5** | **49** | **99,5 %** | **90 %** |

Notes :

- Le total compte trois doublons entre rapports : l'étiquette d'application dérivée (comptée manquante côté transport et côté contrôle), la trame de miroir malformée (contrôle et store), et le texte des avertissements de version du CLI (options et transport). Hors doublons : 4 absences et environ 87 divergences.
- La première ligne contient 139 lignes qui vérifient seulement qu'un nom existe. Sans elles, le comportement identique ou équivalent tombe à 91 % pour cette ligne et à 89 % au total.
- Ce qui est solide partout : les 50 champs de `ClaudeAgentOptions` (49 présents, mêmes valeurs par défaut), tous les drapeaux de ligne de commande (même nom, même format de valeur, même condition, même ordre), les 9 requêtes de contrôle sortantes, les 10 événements de hook et leurs 21 sorties, l'automate de fin d'exécution, les règles de reconstruction de la chaîne de messages, les 14 contrats de conformité du store.

## 4. Les écarts, classés par sens

### 4.1 Rust plus robuste que Python

Python plante ou s'arrête, Rust gère le cas.

- **Messages inconnus conservés.** Un type de message inconnu devient `Message::Unknown` avec son JSON brut ; Python renvoie `None` et le client le saute (`message_parser.rs:51`, `message_parser.py:397-401`). Même logique pour un bloc de contenu inconnu (`message_parser.rs:270`).
- **Trames malformées tolérées.** Une requête de contrôle sans `request_id`, ou une trame de miroir sans `filePath`, tue le lecteur Python par une `KeyError` (`query.py:408`, `query.py:462`). Rust journalise et continue (`control/handlers.rs:30`, `control/mod.rs:218`).
- **Requêtes en attente libérées tout de suite.** Quand la sortie du CLI se termine proprement, Rust fait échouer aussitôt les requêtes de contrôle en attente (`control/mod.rs:307`). Python les laisse attendre leur délai de 60 s.
- **Reconnexion propre.** `connect` sur un client déjà connecté ferme d'abord l'ancienne session (`client.rs:157`). Python écrase l'ancienne sans la fermer (`client.py:208`).
- **Arbre de processus.** Rust tue le CLI et ses descendants (Job Object sous Windows, groupe de processus sous Unix, signal à la mort du parent sous Linux : `process/sys.rs:76`, `:92`, `:237`). Python ne signale que l'enfant direct et compte sur un nettoyage à la sortie de l'interpréteur.
- **Outils MCP.** Deux outils du même nom sont refusés à la construction (`mcp/server.rs:106-114`) ; Python liste les deux et n'appelle que le dernier (`__init__.py:605`). Un outil qui panique renvoie un résultat d'erreur au lieu de casser le serveur (`mcp/server.rs:172`).
- **Contrôle de conformité plus strict.** Le contrat 14 échoue aussi si un store renvoie deux fois le même résumé (`testing/conformance.rs:335`).

### 4.2 Rust plus fidèle au CLI que Python

- **Nom du dossier de projet.** Le CLI tourne en JavaScript, dont les chaînes comptent en unités UTF-16 (un emoji vaut deux unités). Rust compte de même (`sessions/paths.rs:29-73`), Python compte un seul caractère (`sessions.py:84-114`). Pour un chemin contenant un emoji, Python calcule un nom que le CLI ne crée pas et ne retrouve donc pas les sessions. Ce point est validé contre le CLI 2.1.325 dans `docs/validation/project-dir-naming.md`.
- **Horodatages écrits.** Rust écrit des millisecondes, comme `Date.toISOString` (`sessions/timestamp.rs:120-131`). Python écrit des microsecondes (`session_mutations.py:402`).
- **Octets écrits par renommage, étiquette et fork.** Rust écrit l'UTF-8 brut, Python échappe tout ce qui n'est pas ASCII. Que le CLI écrive de l'UTF-8 brut est une inférence (comportement par défaut de `JSON.stringify`), non vérifiée ici.

### 4.3 Rust plus strict

Rust refuse ce que Python laisse passer. C'est en général sain, mais c'est un changement visible.

- Un champ obligatoire du mauvais type JSON donne une erreur `Invalid field in ... message` (`message_parser.rs:154-196`). Python accepte n'importe quelle valeur.
- Un contenu de message utilisateur qui n'est ni un texte ni une liste donne une erreur (`message_parser.rs:286-291`).
- `connect` après `close` donne « Transport is closed » (`transport/subprocess.rs:177`) ; Python peut se reconnecter. `read_messages` ne peut être appelé qu'une fois.
- Un identifiant de session suivi d'un saut de ligne est refusé (`sessions/mod.rs:140-148`) ; l'expression régulière de Python l'accepte.
- La conversion d'un chemin en clé de store refuse `..` et compare en respectant la casse (`session_store.rs:36-44`).

### 4.4 Écarts à risque réel pour qui migre depuis Python

1. **Validation des arguments d'outil désactivée par défaut.** Python valide toujours les arguments contre le schéma JSON avant d'appeler l'outil (`__init__.py:630`). Rust ne le fait pour un schéma brut que si la fonctionnalité Cargo `jsonschema` est activée, et elle ne l'est pas par défaut (`mcp/server.rs:155-163`, `Cargo.toml:14-19`). Les outils typés (struct Rust) restent validés par la désérialisation.
2. **Hiérarchie d'erreurs aplatie.** En Python, `ResultError` hérite de `ProcessError` et `CLINotFoundError` de `CLIConnectionError` (`_errors.py:14`, `:56`) : attraper le parent attrape l'enfant. En Rust ce sont des variantes sœurs d'un seul `enum` (`error.rs`), donc un `match` sur `Error::Process` ne voit pas `Error::ResultError`. Seul `exit_code()` couvre les deux.
3. **Échec d'écriture des identifiants à la reprise.** Si l'écriture de `.credentials.json` dans le dossier temporaire échoue, Python abandonne la reprise et nettoie (`session_resume.py:452`, `:183-191`). Rust journalise un avertissement et lance quand même le CLI, qui échouera plus tard sur l'authentification avec un message moins clair (`resume.rs:439-441`).
4. **`modelUsage` typé sans copie brute.** Python garde le dictionnaire tel quel (`message_parser.py:325`). Rust le convertit en struct à champs fixes et perd toute clé inconnue (`message_parser.rs:417-435`) : un nouveau champ ajouté par le CLI devient invisible. C'est le seul endroit du parseur sans copie brute.
5. **Ctrl+C du terminal.** Rust place le CLI dans son propre groupe de processus (`process/sys.rs:76`, `:168`), donc un Ctrl+C dans le terminal n'atteint plus le CLI directement. Python le laisse dans le groupe du parent. L'arrêt passe alors par la fermeture du transport, ce qui fonctionne, mais le comportement diffère.
6. **`can_use_tool` avec `permission_prompt_tool_name = "stdio"` explicite.** Python lève une erreur pour toute valeur (`types.py:2362`). Rust accepte cette combinaison précise quand les champs sont posés directement (`options.rs:250-257`).
7. **Avertissement « can_use_tool masqué ».** Python émet un avertissement d'une catégorie filtrable, une fois par processus. Rust écrit une ligne de journal à chaque session, sans catégorie (`control/mod.rs:455-457`). Le registre de surface compte pourtant `CanUseToolShadowedWarning` comme « implémenté » : il pointe vers une méthode qui renvoie le texte.
8. **Détails du protocole MCP.** Vérifié sur le code de `mcp` 1.30.0 : Python annonce `tools: {listChanged: false}` et `experimental: {}`, Rust annonce `{"tools": {}}` (`mcp/server.rs:137`). Une méthode inconnue reçoit `-32602 Invalid request parameters` en Python et `-32601 Method not found: <nom>` en Rust (`mcp/server.rs:192-195`). Python refuse une requête avant `initialize`, Rust la sert. Aucun effet connu sur le CLI, mais ce sont des différences visibles sur le fil.
9. **Détection d'un schéma complet.** Python ne reconnaît un schéma JSON que s'il a `type` et `properties` ; sinon il le lit comme une table nom vers type (`__init__.py:439-444`), ce qui déforme `{"type": "object"}`. Rust transmet tout objet tel quel (`mcp/tool.rs:424-428`). Rust est plus juste, mais un schéma écrit pour Python peut se comporter autrement.
10. **Divers.** Format de `--max-budget-usd` (`5.0` en Rust, `5` en Python pour un entier : `transport/command.rs:54`). Limite de taille de message comptée en octets et non en caractères. Entiers à virgule tronqués et négatifs ramenés à 0 dans le parseur. Variables d'environnement fusionnées sans tenir compte de la casse sous Windows. Un horodatage sans fuseau lu comme UTC (Python : heure locale). L'option `user` change aussi le groupe principal. Sur Unix, si `PATH` n'est pas défini, Rust saute la recherche dans `PATH`.

### 4.5 Écarts cosmétiques

Environ 25 divergences ne portent que sur le texte d'un message : « Claude Code » devient « Claude CLI », les messages de validation des noms de skills (guillemets, tiret, conseil `Skills::All`), le détail des erreurs de chargement des réglages, de reprise, de délai du miroir et de validation d'arguments. Le type d'erreur et la condition sont les mêmes. À connaître seulement si un programme compare ces textes.

## 5. Ce qui manque et ce que Rust ajoute

**Manquant (4)**

| Élément | Portée |
|---|---|
| Étiquette d'application dérivée (`_client_app.py`) | Inerte en Python aujourd'hui : son seuil de version est `999.0.0` (`subprocess_cli.py:51`). Non-objectif déclaré. Seule l'option explicite `client_app` est portée. |
| `CanUseToolShadowedWarning` en tant que type | Remplacé par une méthode qui renvoie le texte. |
| Option `debug_stderr` | Dépréciée et jamais lue en Python. |
| Champs d'annotation d'outil supplémentaires | Python les laisse passer (`extra="allow"`, `__init__.py:213`). La struct Rust a des champs fixes. |

**En plus (49), les principaux**

- Macro `#[tool]` qui génère un outil depuis une fonction, et pont vers le crate `rmcp` pour brancher un serveur MCP existant.
- Variante `Unknown` sur chaque énumération du fil, statuts MCP et usage de contexte typés.
- Constructeur d'options (`builder`) qui valide dès `build()`.
- Arrêt de tout l'arbre de processus, nettoyage au `Drop`, verrou sur `connect` et `disconnect`.
- Exemple `session_store_adapter.rs` : un session store complet écrit de zéro.

## 6. Parité des tests

### 6.1 Tests unitaires

Le registre `parity/tests.toml` liste les 1170 tests Python. Recomptage indépendant (analyse syntaxique des fichiers Python) : 1170, sans oubli ni entrée en trop. `cargo xtask parity-check` passe.

| | Nombre | Part |
|---|---|---|
| Couverts par un test Rust | 933 | 79,7 % |
| Exclus : hors produit (packaging 78, adaptateurs d'exemple 48, étiquette dérivée 27, fichiers du dépôt amont 6) | 159 | 13,6 % |
| Exclus : propres à Python (typage 37, runtime asynchrone 25, autres 16) | 78 | 6,7 % |

En retirant les 159 tests hors produit, 933 tests sur 1011 sont couverts, soit 92 %.

**Qualité de la correspondance (échantillon lu des deux côtés)**

- 52 paires lues : 48 équivalentes, 4 plus faibles, 0 qui teste autre chose.
- 38 exclusions lues : 38 justifiées, 0 qui cache un comportement attendu.

Les 4 paires plus faibles :

| Test Python | Ce que le test Rust ne vérifie pas |
|---|---|
| `test_empty_entries_batch_skips_append` | Que `append` n'est jamais appelé pour un lot vide : il vérifie seulement que le store est vide (`mirror.rs:496-507`). |
| `test_unmapped_file_path_is_dropped_silently` | Qu'aucune erreur de miroir n'est signalée. |
| `test_close_does_not_hang_on_a_tool_blocked_outside_the_event_loop` | L'avertissement journalisé quand la fermeture renonce. |
| `test_new_hook_events_registered_in_hooks_config` | Que les options acceptent ces événements comme clés (trivial). |

**Point faible structurel : le repli de plusieurs tests en un seul.** Les 933 tests Python pointent vers 566 tests Rust distincts. 194 tests Rust servent plusieurs tests Python, jusqu'à 19 pour `store_readers_match_the_disk_readers_for_the_same_content`. J'ai lu ce test : il vérifie que les lecteurs sur store renvoient la même chose que les lecteurs sur disque pour le même contenu. La couverture est réelle mais indirecte : elle dépend des tests des lecteurs sur disque. C'est dans ces replis (sessions, transport, reprise) que d'autres assertions plus faibles sont le plus probables, et c'est la zone la moins échantillonnée.

### 6.2 Tests de bout en bout

Les 48 tests Python ont chacun un test Rust pour le même scénario. Lecture des deux côtés, sans exécution :

| Identique | Rust plus strict | Rust plus faible | Rust seulement |
|---|---|---|---|
| 30 | 15 | 3 | 8 |

- **Plus strict** : Rust échoue là où Python saute le test ou n'affirme rien. Exemples : les trois tests de contrôle dynamique n'ont aucune assertion en Python (`test_dynamic_control.py:79-98`), et cinq tests de réglages passent en Python même si aucun message d'initialisation n'arrive.
- **Plus faible** : `error_results` ne compare pas `session_id`, `terminal_reason` et `uuid` entre l'erreur et le résultat (`e2e/error_results.rs:11-43` face à `test_error_results.py:50-59`), alors que les accesseurs existent. Deux autres écarts sont mineurs (`include_partial_messages`, `subagent_session_reads`).
- **Harnais** : Rust fixe le modèle `claude-haiku-5-5` et pose aussi `ANTHROPIC_MODEL`, Python fixe `claude-opus-5` par `ANTHROPIC_DEFAULT_MODEL` seulement. Rust impose 240 s par tour et lance 4 sessions en parallèle.
- **Incohérence** : `tests/store_resume_e2e.rs` ne passe pas par le harnais commun. Sans `CLAUDE_AGENT_KIT_E2E=1`, il sort tôt et passe en silence (`:94-97`), alors que le reste de la suite échoue quand les identifiants manquent.

### 6.3 Exemples

19 fichiers d'exemple Python, 12 côté Rust.

| Identique | Rust plus riche | Rust plus pauvre | Absent en Rust | Rust seulement |
|---|---|---|---|---|
| 5 | 5 | 1 | 8 | 1 |

- Absents mais la fonctionnalité existe : `tools_option`, `plugin_example`, `filesystem_agents`.
- Absents par choix ou sans objet : les trois adaptateurs de store (Postgres, Redis, S3 : non-objectif), `streaming_mode_ipython` et `streaming_mode_trio` (propres à Python).
- Plus pauvre : `interactive_client.rs` couvre 4 des 10 scénarios environ de `streaming_mode.py`.
- L'exemple Rust `hooks.rs` corrige un défaut de l'exemple Python, qui enregistre un hook `UserPromptSubmit` mais renvoie `hookEventName: "SessionStart"`.

## 7. Vérification des rapports

Chaque rapport a été recontrôlé en relisant les lignes citées dans les deux dépôts. Priorité donnée aux écarts déclarés, puisque ce sont eux qui portent les conclusions.

| Rapport | Recontrôlé par l'agent principal | Résultat |
|---|---|---|
| Options, configuration, erreurs | Hiérarchie d'erreurs, les 2 absences, le cas `can_use_tool` avec `"stdio"`, l'entrée du registre pour `CanUseToolShadowedWarning` | Confirmé |
| Messages et parseur | 9 des 12 divergences | Confirmé, avec une nuance (ci-dessous) |
| Transport | 8 des 18 divergences, le seuil `999.0.0`, l'ordre des variables d'environnement | Confirmé |
| Protocole de contrôle | 8 des 11 divergences | Confirmé |
| MCP | 5 divergences dans le code, 3 autres sur le code source de `mcp` 1.30.0 téléchargé pour l'occasion | Confirmé ; 3 restent non vérifiées |
| Sessions | 6 des 13 divergences, la note de validation du nom de dossier | Confirmé, avec un changement de sens (ci-dessous) |
| Store, reprise, miroir | 9 des 12 divergences, l'existence de l'import | Confirmé |
| Registre de tests | Une paire plus faible, le test le plus replié, les totaux par script | Confirmé ; échantillon sous la cible (52 paires au lieu de 60) |
| E2E et exemples | `error_results`, le harnais, `store_resume_e2e`, la liste des exemples | Confirmé |

Aucune affirmation recontrôlée ne s'est révélée fausse. Trois ajustements apportés aux rapports :

1. **Sens d'un écart.** Le rapport sur les sessions classait le comptage UTF-16 comme une divergence neutre. Rapproché de `docs/validation/project-dir-naming.md`, c'est Python qui s'écarte du CLI.
2. **Portée d'un écart.** L'absence de variante inconnue sur `HookSpecificOutput` ne gêne que la désérialisation. Or cette valeur est construite par l'application et seulement sérialisée : l'effet pratique est nul.
3. **Registre trop généreux sur un point.** `parity/surface.toml` annonce 100 % d'exports implémentés, mais l'entrée `CanUseToolShadowedWarning` pointe vers une méthode et non vers un type. La note du registre le dit ; le pourcentage ne le reflète pas.

Deux sous-agents (registre de tests, e2e) ont atteint leur limite de tours avant de rendre un rapport. Ils ont été relancés avec un budget borné et ont terminé.

## 8. Recommandations

Par ordre d'intérêt pour un utilisateur qui vient de Python.

1. **Validation des arguments d'outil.** Activer `jsonschema` par défaut, ou documenter en tête de `SdkMcpTool::raw` que les arguments ne sont pas validés sans cette fonctionnalité.
2. **Erreurs.** Ajouter des prédicats qui reproduisent la hiérarchie Python (par exemple `is_process()` vrai pour `Process` et `ResultError`, `is_cli_connection()` vrai pour `CliConnection` et `CliNotFound`), et l'écrire dans `docs/migration.md`.
3. **Reprise.** Faire échouer la reprise quand l'écriture des identifiants échoue, comme Python.
4. **`modelUsage`.** Garder une copie brute ou un champ `extra`, comme le font déjà les autres types du fil.
5. **Tests.** Reprendre les 4 assertions plus faibles de la section 6.1 et les 3 comparaisons manquantes de `error_results`. Faire passer `store_resume_e2e.rs` par le harnais commun.
6. **Exemples.** Ajouter `tools`, `plugins` et les agents chargés depuis le système de fichiers.
7. **À trancher.** Le groupe de processus séparé (Ctrl+C), l'avertissement journalisé à chaque session, et l'acceptation de `can_use_tool` avec `"stdio"` explicite sont des choix défendables, mais ils devraient figurer dans `docs/migration.md`.

## 9. Limites

- **Lecture statique.** La comparaison porte sur le code. Les tests sur le vrai CLI n'ont pas été lancés, côté Rust comme côté Python.
- **Comptes approchés.** Les totaux de la section 3 additionnent des décomptes de grain différent, dont certains donnés comme approximatifs.
- **Lignes « identique » peu recontrôlées.** La vérification a porté sur les écarts. Les lignes classées identiques reposent sur la lecture des sous-agents, sur les 808 tests hors ligne qui passent, et sur les extraits relus en passant.
- **MCP.** Trois divergences restent non vérifiées : `tools/call` sans nom, la présence de `maxResultSizeChars` dans `annotations` en plus de `_meta`, et le texte d'erreur d'un message malformé. La vérification a porté sur `mcp` 1.30.0 ; la branche 2.x, que Python accepte aussi, n'a pas été lue.
- **Bibliothèque standard Python.** Plusieurs écarts de bord reposent sur un comportement supposé de la bibliothèque standard (décodage UTF-8 strict, repli de `shutil.which`, texte vide de `TimeoutError`).
- **Plateformes.** Les branches macOS et Linux ont été lues, pas exécutées. La machine de travail est sous Windows.
- **Registre de tests.** Les fichiers `test_transport.py` et `test_session_resume.py` n'ont qu'une paire lue chacun. Quatre motifs d'exclusion (les 159 tests hors produit) n'ont pas été lus : ils sont exclus par nature.
- **Amont mouvant.** La mesure vaut pour le commit `4720d36`. `cargo xtask upstream-diff <tag>` liste ce qui change ensuite.
