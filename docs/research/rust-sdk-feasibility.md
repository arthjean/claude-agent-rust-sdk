# Faisabilité d'un SDK Rust pour Claude Agent, à partir du SDK Python

> Date : 2026-11-14. Base : `claude-agent-sdk-python` au commit `4720d36` (SDK v0.2.182, CLI épinglé 2.1.325).
> Méthode : 5 sous-agents sur le code (un par sous-système), 1 sur le web, chaque rapport recontrôlé contre le code ou la source d'origine. Les corrections apportées aux rapports sont listées en section 9.

## 1. Verdict

**Faisable, sans obstacle technique.** Le SDK Python ne contient aucune liaison native avec Claude Code : il lance le CLI `claude` comme processus enfant et échange du NDJSON (une ligne = un objet JSON) sur stdin/stdout. Tout le reste est de la logique locale : typage des messages, routage, fichiers de session. Rien de cela ne dépend de Python.

Preuve directe : un script de 50 lignes sans le SDK (`subprocess` + `json` seulement) a lancé le CLI 2.1.325 avec les drapeaux fixes du SDK, envoyé une requête `initialize`, et reçu `control_response / success` avec 23 clés dont `capabilities`, `claude_code_version`, `hook_on_failure_supported`, puis sortie propre (code 0). Aucun prompt envoyé, donc aucun appel modèle.

Le coût réel est ailleurs :

1. **La surface** : 13 707 lignes de source, 159 noms exportés, 50 options, environ 45 drapeaux CLI, 13 sous-types de contrôle, et 1 171 tests unitaires (27 684 lignes) qui fixent le comportement.
2. **La maintenance** : le protocole de contrôle n'a aucune spécification officielle ni promesse de stabilité, et le CLI sort une version tous les quelques jours. Le SDK Python est republié à chaque version du CLI (`RELEASING.md:32-43`).

## 2. Ce que le SDK est vraiment

```
application  ->  options (50 champs)  ->  argv + env  ->  processus `claude`
                                                            | stdin  : user / control_request / control_response
                                                            | stdout : messages + control_request + control_response
             <-  messages typés  <-  parseur  <-  routeur  <-+
```

Deux canaux logiques sur le même tuyau :

- **Messages** : `user`, `assistant`, `system`, `result`, `stream_event`, `rate_limit_event`, `conversation_reset`.
- **Protocole de contrôle** (requêtes/réponses dans les deux sens, corrélées par `request_id`) : c'est par là que passent les hooks, la demande de permission d'outil, et les serveurs MCP hébergés dans le processus de l'application.

## 3. Inventaire vérifié par sous-système

| Sous-système | Fichiers Python | Lignes | Dépend de Python ? | Équivalent Rust |
|---|---|---|---|---|
| Transport et processus | `_internal/transport/subprocess_cli.py` | 1 193 | Non | `tokio::process`, tuyaux, cadrage par lignes |
| Routeur et protocole de contrôle | `_internal/query.py`, `client.py`, `_internal/client.py` | 2 099 | Non (anyio seulement) | tâche lectrice + `mpsc` + `oneshot` |
| Compatibilité asyncio/trio | `_internal/_task_compat.py` | 188 | Oui, entièrement | **Disparaît** (un seul runtime) |
| Types et parseur | `types.py`, `_internal/message_parser.py` | 3 387 | Typage dynamique | `serde` (enums étiquetées) |
| MCP en processus | `_internal/sdk_mcp_bridge.py`, `_mcp_compat.py`, partie de `__init__.py` | ~900 | Paquet `mcp` + introspection de types | répartiteur JSON-RPC natif, ou `rmcp` |
| Sessions | `sessions.py` + 8 fichiers | ~5 000 | Non (fichiers + magasin) | `std::fs`, trait `SessionStore` |
| Empaquetage du CLI | `scripts/download_cli.py`, `build_wheel.py` | ~1 200 | Spécifique aux wheels | À repenser (section 6.6) |

### 3.1 Transport (vérifié)

- Interface `Transport` à 6 méthodes : `connect`, `write`, `read_messages`, `close`, `is_ready`, `end_input` (`transport/__init__.py:22-84`).
- Recherche du CLI : `cli_path`, puis binaire embarqué `_bundled/claude`, puis `PATH`, puis une liste de dossiers connus (`subprocess_cli.py:251-343`).
- Environnement fusionné dans cet ordre : hérité sans `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT=sdk-py`, `options.env`, puis `CLAUDE_AGENT_SDK_VERSION` non surchargeable (`subprocess_cli.py:764-770`). S'y ajoutent `CLAUDE_CODE_SDK_READS_SESSION_STATE=1`, `CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING`, `PWD`, et la propagation OpenTelemetry (`:785-823`).
- Le prompt et les agents ne passent jamais par la ligne de commande : ils partent sur stdin. Il n'y a donc pas de limite de longueur de commande à gérer.
- Cadrage : découpe sur `\n` uniquement, lignes ne commençant pas par `{` ignorées, plafond de 1 Mio par message (`:42`, `:1065-1083`).
- Fermeture bornée : fermer stdin, attendre 5 s, `terminate`, attendre 5 s, `kill`, attendre 5 s (`:984-1001`). Le délai de grâce laisse le CLI écrire son fichier de session.
- Version : lue dans le premier `system/init` (`claude_code_version`), jamais via `claude -v` (`:1153-1175`).
- Windows : refus des CLI `.bat`/`.cmd` (injection de commandes, CVE-2024-27980) et des métacaractères `cmd.exe` dans les valeurs de reprise (`:60-66`, `:346-449`).

### 3.2 Protocole de contrôle (vérifié)

Enveloppe sortante : `{"type":"control_request","request_id":"req_{n}_{8 hex}","request":{subtype,...}}`, délai 60 s (`query.py:735-780`).
Réponse du SDK : `{"type":"control_response","response":{"subtype":"success"|"error","request_id",...}}` (`query.py:709-733`).

| Sens | Sous-types |
|---|---|
| SDK vers CLI (10) | `initialize`, `interrupt`, `set_permission_mode`, `set_model`, `rewind_files`, `mcp_reconnect`, `mcp_toggle`, `stop_task`, `mcp_status`, `get_context_usage` |
| CLI vers SDK (3) | `can_use_tool`, `hook_callback`, `mcp_message` |
| Autres trames | `control_cancel_request` (annule un gestionnaire en cours), `transcript_mirror` (copie des transcriptions vers un magasin) |

Points de comportement à reproduire :

- File de messages bornée à 100 ; quand elle est pleine, la lecture se bloque (`query.py:234-236`).
- Hooks : identifiants `hook_{n}` attribués à l'initialisation ; seules les clés `async_` et `continue_` sont renommées en sortie (`query.py:118-134`, `:327-347`).
- Permission : `allow` renvoie `updatedInput` (l'entrée d'origine par défaut), `deny` renvoie `message` et éventuellement `interrupt` (`query.py:646-667`).
- **Fermeture de stdin** : la partie la plus subtile. Si des hooks, `can_use_tool` ou un serveur MCP en processus sont configurés, stdin doit rester ouvert jusqu'à la fin réelle du travail. La fin est déduite des trames `result`, de `session_state_changed` (`idle`/`running`), d'un registre des tâches d'arrière-plan (`local_agent`, `local_workflow`) et d'un plafond de 10 minutes réglable par `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS` (`query.py:57-87`, `:899-1120`).
- Erreurs : un code de sortie non nul précédé d'un `result` en erreur devient une `ResultError` portant le texte utile (`query.py:90-115`).

### 3.3 Types et parseur (vérifié par comptage AST)

- `__all__` : **159 noms** (24 fonctions, 6 erreurs, le reste en types et classes).
- `ClaudeAgentOptions` : **50 champs** (`types.py:2391`).
- `types.py` : 31 classes de données (objets construits par le parseur), 89 autres classes (presque toutes des `TypedDict`, c'est-à-dire des dictionnaires qui gardent les noms de clés du fil), 1 `Protocol` (`SessionStore`).
- Parseur : champ requis manquant = erreur ; type de message inconnu = ignoré (`None`) ; sous-type `system` inconnu = message générique avec le dictionnaire brut ; bloc de contenu inconnu = abandonné en silence (`message_parser.py:95-131`, `:397-401`).
- Ensembles fermés vérifiés : `PermissionMode` (6 valeurs), `HookEvent` (10), `EffortLevel` (5), `SettingSource` (3).
- Beaucoup de charges utiles restent non typées : `usage`, `ToolUseBlock.input`, `StreamEvent.event`, `structured_output`, `tool_use_result`.

### 3.4 MCP en processus (vérifié)

- Le CLI reçoit `--mcp-config` avec `{type:"sdk", name}` ; les messages JSON-RPC (l'enveloppe requête/réponse de MCP) arrivent ensuite dans des `mcp_message`.
- Côté Python, le pont pilote le vrai `mcp.server.Server` sur un flux en mémoire ; il ne réimplémente pas les méthodes (`sdk_mcp_bridge.py:1-22`, `:141-190`).
- Une notification reçoit quand même l'accusé `{"jsonrpc":"2.0","result":{}}` ; serveur inconnu = erreur `-32601` ; exception = `-32603` (`query.py:699-811`).
- Résultats d'outils : seuls `text` et `image` passent tels quels ; les ressources texte sont aplaties en texte ; outil inconnu, arguments invalides et exceptions reviennent tous en résultat `isError`, jamais en erreur de protocole (`__init__.py:471-514`, `:620-641`).
- `maxResultSizeChars` voyage dans `_meta["anthropic/maxResultSizeChars"]` (`__init__.py:459-468`).

### 3.5 Sessions (vérifié)

- Logique purement locale : lecture, listage, renommage, étiquetage, suppression, duplication et import de transcriptions JSONL sous `<config>/projects/<chemin-assaini>/<uuid>.jsonl`. Aucune conversation avec le CLI.
- Nom du dossier projet : tout caractère non alphanumérique devient `-` ; au-delà de 200 caractères, troncature plus un hachage `h = h*31 + c` sur 32 bits signés, écrit en base 36 (`sessions.py:81-114`). Le CLI utilise un autre hachage (`Bun.hash`), d'où un repli par préfixe (`sessions.py:157-184`).
- Métadonnées lues sans analyser tout le JSON : 64 Kio en tête et en queue de fichier, recherche de chaînes (`sessions.py:33`).
- `SessionStore` : 2 méthodes requises (`append`, `load`), 4 optionnelles ; une suite de conformité publique (`testing/session_store_conformance.py`).
- Reprise depuis un magasin : le SDK fabrique un faux dossier de configuration temporaire (`mkdtemp("claude-resume-")`), y copie les identifiants sans `refreshToken`, et y pointe le CLI par `CLAUDE_CONFIG_DIR` (`session_resume.py:167`, `:432-447`).
- Miroir : lots de 500 entrées ou 1 Mio, 3 tentatives avec attente de 0,2 s puis 0,8 s, délai de 60 s (`transcript_mirror_batcher.py:28-35`).

## 4. Ce qui existe déjà (vérifié sur crates.io le 2026-11-14)

- **Pas de SDK Rust officiel.** La doc dit : « programmable in Python and TypeScript » et, pour les autres langages, « run the CLI as a subprocess » ([overview](https://code.claude.com/docs/en/agent-sdk/overview)).
- Le nom `claude-agent-sdk` est pris sur crates.io (v0.1.1, 2025-09-30, 4 331 téléchargements) par un particulier ; le dépôt qu'il déclare, `github.com/anthropics/claude-agent-sdk-rust`, renvoie 404.

| Crate | Version, date | Téléchargements | Sessions ? |
|---|---|---|---|
| `anthropic-agent-sdk` | 0.2.75, 2025-12-23 | 88 153 | Non |
| `claude-agent-sdk-rs` | 0.6.4, 2026-02-09 | 35 202 | Non |
| `claude-code-sdk-rust` | 0.4.1, 2026-07-21 | 6 341 | Oui (d'après son README) |
| `seher-claude-agent-sdk` | 0.0.59, 2026-08-25 | 2 257 | Non vérifié |

Lecture : une dizaine de portages communautaires, aucun à jour. Le plus téléchargé n'a pas bougé depuis 11 mois alors que le SDK Python en est à v0.2.182. La couverture de ces crates vient de leurs README, pas d'une lecture de leur code. **Le manque à combler est la tenue à jour, pas l'existence d'un portage.**

- `rmcp` (SDK MCP officiel en Rust) : 3.10.0 du 2026-11-12, accepte n'importe quel transport, dont un transport en processus (`transport-worker`) ([dépôt](https://github.com/modelcontextprotocol/rust-sdk)).

## 5. Contraintes non techniques (citations vérifiées)

- **Protocole non spécifié.** La doc ne décrit que le format de sortie `stream-json` et un tableau `capabilities` dans l'événement d'initialisation : « Check it to feature-detect instead of comparing version strings » ([headless](https://code.claude.com/docs/en/headless)). Les sources Python et TypeScript sont la seule référence.
- **Authentification.** « Anthropic does not allow third party developers to offer claude.ai login or rate limits for their products » sauf accord préalable. Une bibliothèque n'est pas concernée ; un produit bâti dessus doit utiliser une clé API ([overview](https://code.claude.com/docs/en/agent-sdk/overview), [legal](https://code.claude.com/docs/en/legal-and-compliance)).
- **Nom.** « Claude Code » et « Claude Code Agent » sont interdits dans un nom de produit ; « Claude Agent » et « {Nom} Powered by Claude » sont permis. Aucune règle propre aux noms de paquets n'a été trouvée.
- **Binaire.** « The Claude Code binary must not be modified. » Aucune source ne permet ni n'interdit explicitement qu'un tiers le redistribue.
- **Licence du SDK Python** : MIT, usage soumis aux conditions commerciales d'Anthropic.

## 6. Ce qui doit être fait

### 6.1 Transport

- Lancer le processus avec `tokio::process::Command` (argv en liste, jamais de shell), tuyaux stdin/stdout, stderr conditionnel.
- Reproduire le constructeur de commande : la table des ~45 drapeaux (`subprocess_cli.py:519-744`), y compris la forme `--flag=valeur` pour les valeurs qui pourraient être prises pour un drapeau.
- Reproduire la fusion d'environnement et la validation des noms de skills.
- Cadrage NDJSON avec plafond de taille, verrou d'écriture, fermeture en escalade 5 s / 5 s / 5 s.
- **Différence de fond** : en Python, la fermeture est « blindée » contre l'annulation. En Rust, annuler une tâche revient à la lâcher (`drop`), et il n'existe pas de destructeur asynchrone. Il faut donc une méthode `close().await` explicite, plus un filet de sécurité à la destruction (`kill_on_drop`, et idéalement un Job Object sous Windows et `PR_SET_PDEATHSIG` sous Linux pour tuer l'enfant si le parent meurt). Python fait cela avec `atexit` (`subprocess_cli.py:72-82`).
- Garder les protections Windows.

### 6.2 Routeur et protocole de contrôle

- Une tâche lectrice qui classe chaque trame ; canal borné à 100 vers le consommateur ; un `oneshot` par requête de contrôle en attente ; une table des gestionnaires en cours pour honorer `control_cancel_request`.
- Les 13 sous-types, la poignée de main `initialize` (délai : au moins 60 s), la vérification `hook_on_failure_supported`.
- La machine à états de fin d'exécution (section 3.2). C'est le morceau le plus risqué : 2 715 lignes de tests rien que pour `query.py`.
- Deux façades publiques : `query()` (flux de messages) et un client interactif avec ses 15 méthodes publiques (`client.py`).

### 6.3 Types

- Modéliser avec `serde` : enums étiquetées par `type`, `subtype`, `hook_event_name`, `behavior`.
- Décider la compatibilité ascendante dès le départ : variante de repli pour les types inconnus, `#[non_exhaustive]`, conservation des champs inconnus. Python jette les blocs inconnus ; un SDK Rust peut faire mieux en les gardant bruts.
- Garder `serde_json::Value` pour les charges non typées.
- Options : un constructeur (builder) pour les 50 champs, avec les unions « chaîne ou structure » (`system_prompt`, `tools`, `mcp_servers`, `skills`) en enums.
- Rappels (hooks, permission) : fermetures asynchrones partagées entre tâches (`Arc<dyn Fn ... + Send + Sync>`).
- `HookJSONOutput` n'a pas de champ discriminant (la forme asynchrone se reconnaît à la présence de `async`) : à modéliser à la main.

### 6.4 MCP en processus

Deux voies :

- **A. Répartiteur JSON-RPC natif** : `initialize`, `ping`, `tools/list`, `tools/call`, accusé des notifications. Quelques centaines de lignes, aucune dépendance, contrôle total.
- **B. `rmcp`** sur un transport en mémoire : c'est le miroir exact de l'approche Python, et cela ouvre ressources et prompts, mais lie le SDK à une crate qui publie chaque semaine.

Dans les deux cas : générer le schéma JSON depuis une structure Rust (`schemars`) remplace la conversion de types Python, et la désérialisation `serde` remplace la validation `jsonschema` pour les outils typés.

### 6.5 Sessions

- Portage de logique pure, le plus gros en volume (~5 000 lignes, ~8 300 lignes de tests).
- Normalisation Unicode NFC/NFKC (crate `unicode-normalization`).
- Trait `SessionStore` asynchrone. Python détecte les méthodes optionnelles par introspection de classe (`session_store_validation.py:8-15`) ; en Rust il faut un mécanisme explicite (drapeaux de capacités ou sous-traits).
- Porter la suite de conformité comme fonction publique.
- Reprise : dossier temporaire, copie filtrée des identifiants et réglages, lecture du trousseau macOS via la commande `security`.

### 6.6 Distribution du CLI

- Le wheel Python embarque le binaire, et frôle la limite PyPI de 250 Mio par fichier (`RELEASING.md:19`). crates.io plafonne une crate à 10 Mo par défaut (chiffre cité de mémoire, non revérifié ce jour) : **embarquer le binaire est exclu**.
- À faire : recherche sur `PATH` et dossiers connus, plus un installeur optionnel qui télécharge depuis `downloads.claude.ai/claude-code-releases/$VERSION/manifest.json` et vérifie la somme SHA256 et la signature GPG (disponible depuis 2.1.89, [setup](https://code.claude.com/docs/en/setup)). Le script Python, lui, ne vérifie aucune somme dans son propre code (`scripts/download_cli.py`).
- Plateformes : macOS, Linux (glibc et musl), Windows, en x64 et arm64.

### 6.7 Tests et suivi de l'amont

- Un faux CLI scriptable (rejoue du NDJSON) pour porter les 1 171 tests unitaires sans réseau.
- Une suite de bout en bout avec clé API (49 tests côté Python).
- Un mécanisme de détection de dérive : à chaque version du SDK Python, comparer `types.py`, `subprocess_cli.py` et `query.py` à la version précédente. Sans cela, le portage vieillit comme les crates existantes.
- Détection par `capabilities` plutôt que par numéro de version, comme le recommande la doc.

## 7. Pièges identifiés

1. **Unités de longueur.** Python compte en points de code Unicode ; le CLI est en JavaScript, qui compte en unités UTF-16. Pour le seuil de 200 caractères et le hachage des chemins, Rust devra probablement itérer avec `encode_utf16()`. *Inférence non vérifiée* : le code Python lui-même itère en points de code (`sessions.py:84-86`), donc il diverge peut-être déjà du CLI pour les caractères hors plan de base.
2. **Fermeture de stdin** trop précoce : les hooks et outils MCP des agents d'arrière-plan cessent de répondre.
3. **Annulation** : pas d'équivalent direct de la portée blindée d'anyio.
4. **Renommages de champs** disséminés (snake_case côté Python, camelCase côté fil, parfois les deux dans le même objet, par exemple `SystemInitData`).
5. **Couplage aux fichiers internes du CLI** : format des transcriptions, nom des dossiers, fichiers `.meta.json` des sous-agents. Rien de cela n'est documenté.
6. **Plafond de 1 Mio** : Python compte des caractères alors que le message d'erreur parle d'octets (`subprocess_cli.py:1065-1073`). Choisir et documenter.

## 8. Décisions ouvertes, avec recommandation

| Décision | Recommandation | Raison |
|---|---|---|
| Runtime asynchrone | `tokio` seul | Supprime tout `_task_compat.py` ; `tokio::process` est le standard |
| MCP | Voie A native, voie B derrière une option de compilation | Indépendance vis-à-vis d'une crate à cadence hebdomadaire, sans fermer la porte aux serveurs `rmcp` existants |
| Périmètre de la première version | Transport, contrôle, types, hooks, permissions, MCP ; sessions ensuite | Les sessions sont indépendantes du reste et représentent un tiers du volume |
| Nom | Éviter `claude-code-*` ; `claude-agent-sdk` est pris | Règles de marque (section 5) |
| Référence de comportement | Suivre aussi le SDK TypeScript | Plusieurs commentaires Python disent « comme le SDK TypeScript » ; c'est lui qui précède |
| Types | Écrits à la main d'abord | Aucun schéma amont à partir duquel générer |

## 9. Vérification et limites

**Contrôles effectués par l'agent principal :**

- Relecture directe des passages porteurs : `subprocess_cli.py:36-85, 746-870, 917-1190` ; `query.py:50-150, 225-465, 606-900` ; `_task_compat.py:140-188` ; `sdk_mcp_bridge.py:1-260` ; `__init__.py:364-647` ; `message_parser.py:50-139, 376-401` ; `sessions.py:28-187`.
- Comptage par analyse syntaxique : 159 exports, 50 champs d'options, valeurs des ensembles fermés.
- Liste des drapeaux CLI recoupée par recherche de tous les `cmd.append/extend`.
- Web : métadonnées crates.io relues par l'API (chiffres identiques), 404 du dépôt confirmé, citations retrouvées mot pour mot dans les pages officielles.
- Sonde du protocole sur le CLI local (via le lanceur installé sur cette machine).

**Corrections apportées aux rapports :**

- Types : le rapport annonçait « environ 60 classes de données et 80 TypedDict ». Mesure : 31 et environ 89.
- Sessions : le hachage était qualifié de « style djb2 ». C'est `h*31 + c` (style `hashCode` de Java).
- Transport : « aucune somme de contrôle » est exact pour le code du SDK, mais le canal officiel publie un manifeste signé.

Aucune autre erreur trouvée sur les points contrôlés.

**Non couvert :**

- Le code du SDK TypeScript et celui du CLI (fermé) n'ont pas été lus.
- Les fichiers de tests ont été lus par sondage, pas en entier.
- Le code des crates Rust concurrentes n'a pas été lu.
- `.github/` (13 workflows) n'a été que listé : c'est la chaîne de publication PyPI, sans incidence sur la faisabilité.
- Le droit de redistribuer le binaire reste sans source.
