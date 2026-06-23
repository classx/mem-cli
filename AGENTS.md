# AGENTS.md

Инструкции для AI-агентов (например, GitHub Copilot CLI), работающих над `mem-cli`.

## Контекст проекта (mem-cli)

Долговременный контекст проекта хранится в `mem-cli` (Rust + SQLite).
Контекст у каждого разработчика личный и хранится вне репозитория. Путь к БД
определяется по приоритету: `MEMORY_DB_DIR` (override) → маркер `.mem-project`
(`${XDG_DATA_HOME:-~/.local/share}/mem/<slug>/`) → `.memory/` (legacy).
Инициализация проекта: `mem-cli init "Имя проекта"`.

- В начале работы прочитать контекст:
  - `mem-cli list facts`
  - `mem-cli decisions`
  - `mem-cli modules`
  - `mem-cli list commands`
  - срез по теме (все сущности сразу): `mem-cli find <tag>`
- По ходу работы дописывать новое:
  - стабильные факты: `mem-cli add facts "..."`
  - принятые технические решения: `mem-cli add decisions "..."`
  - проверенные команды build/test/lint: `mem-cli add commands "..."`
  - описание модулей: `mem-cli add modules "..."`
  - тематическая разметка: `mem-cli tag <entity> <id> <tag>...`,
    снятие — `mem-cli untag <entity> <id> <tag>`; обзор тем — `mem-cli tags`.
- Удаление: `mem-cli delete <entity> <id>` (soft), `mem-cli purge <entity> <id>` (hard).
- Проверка целостности тегов/БД: `mem-cli doctor` (`--fix` — безопасная автопочинка).
- Для машиночитаемого вывода добавляйте флаг `--json`.
- Для MCP-клиентов запускать локальный сервер: `mem-cli mcp` (stdio transport).
  Ресурсы: `mem://facts`, `mem://decisions`, `mem://modules`, `mem://commands`.
  Tools: `list_*`, `find_by_tag`, `add_*`, `tag_record`, `untag_record`, `doctor`.

Основной интерфейс к данным — `mem-cli`; прямой SQL только для отладки.

## Сборка, тесты, линт

```sh
make build      # cargo build
make test       # cargo test
make lint       # cargo fmt --check + cargo clippy -- -D warnings
make release    # cargo build --release
```

Перед каждым commit запускать тесты. Перед merge в `main`: build + test + lint.

## Git workflow

- Каждая фаза/feature — отдельный branch; задача в фазе — sub-branch
  `phase-N/X.Y-description`.
- Перед созданием нового branch — метка в главном branch с префиксом `pre_`.
- `main` всегда зелёный (собирается и проходит тесты).
- Merge с `--no-ff`.
- Перед merge в `main`: обновить версию в `Cargo.toml` и добавить запись в
  `CHANGELOG.md` (на английском).
- После merge в `main`: поставить метку `vX.Y.Z`.
- Все коммиты содержат trailer:
  `Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>`

## RFC

- RFC лежат в `docs/rfcs/`, план — `docs/PLAN-0001.md`.
- Управление через `rfc-cli` (`~/.local/bin/rfc-cli`): `new`, `list`, `view`,
  `status`, `set`, `check`.
- Статусы последовательно: draft → review → accepted → implemented.
- После изменения RFC — валидация: `rfc-cli check NNNN`.

## Код

- Минимальные изменения, без побочных правок в несвязанном коде.
- Не добавлять зависимости без явной причины; избегать дублирования кода.
- Использовать существующие компоненты, не плодить дубликаты.

<!-- mem-cli:start -->
## mem-cli context storage

Project context is stored locally per developer, outside the repository:
`${XDG_DATA_HOME:-~/.local/share}/mem/<slug>/project_context.db`.
The project slug (`mem-cli-59eea7ceac446418`) is fixed in the `.mem-project` file.
The path can be overridden via the `MEMORY_DB_DIR` variable.
<!-- mem-cli:end -->



