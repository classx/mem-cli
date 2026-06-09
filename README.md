# mem-cli

`mem-cli` — минимальный CLI на Rust + SQLite для хранения и быстрого
восстановления долговременного контекста проекта между сессиями.

Реализация [RFC-0001](docs/rfcs/0001.md) (план — [PLAN-0001](docs/PLAN-0001.md)).

## Возможности

Четыре сущности контекста:

- `facts` — стабильные факты о проекте;
- `decisions` — принятые технические решения;
- `commands` — проверенные команды build/test/lint;
- `modules` — краткое описание модулей и их ответственности.

## Установка

```sh
cargo build --release
# бинарь: target/release/mem-cli
```

Опционально скопируйте бинарь в каталог из `PATH`:

```sh
cp target/release/mem-cli ~/.local/bin/
```

## Хранилище

Файл БД — `project_context.db`. Контекст у каждого разработчика **личный** и
хранится **вне репозитория**. Каталог определяется по приоритету:

1. переменная окружения `MEMORY_DB_DIR` (override для тестов/CI) — если задана;
2. иначе — по маркеру проекта `.mem-project` (ищется вверх по дереву каталогов,
   как `.git`): `${XDG_DATA_HOME:-~/.local/share}/mem/<slug>/`;
3. иначе (нет маркера) — каталог `.memory/` в текущей директории (legacy).

Слаг проекта (`<имя>-<случайный id>`) фиксируется в `.mem-project` при `init` и
далее неизменен независимо от пути и реклонов. Каталог создаётся автоматически.

Файл `.mem-project` **коммитится в репозиторий**: слаг общий для всей команды,
а сама БД у каждого разработчика остаётся локальной (в `$HOME`, вне репозитория).

## Использование

```sh
# инициализировать проект: создать маркер .mem-project, БД и схему
mem-cli init "Имя проекта"
# имя необязательно — по умолчанию берётся имя корневого каталога репозитория
mem-cli init

# показать путь к БД и статус (OK, если БД существует)
mem-cli info

# добавить записи
mem-cli add facts "проект использует Rust 2024 edition"
mem-cli add decisions "БД хранится в .memory/"
mem-cli add commands "cargo test"
mem-cli add modules "db: слой хранения и миграции"

# вывести записи (таблица по умолчанию)
mem-cli list facts
# вывод в JSON
mem-cli list facts --json

# быстрые алиасы
mem-cli decisions
mem-cli modules --json

# удаление
mem-cli delete facts 1   # soft delete: выставляет deleted_at, скрывает из list
mem-cli purge  facts 1   # hard delete: физически удаляет строку

# обновление (удалить старую запись + добавить новую)
mem-cli update commands 1 "make build"          # soft delete старой + add
mem-cli update commands 1 "make build" --hard   # purge старой + add
```

> Основной интерфейс работы с данными — `mem-cli`. Прямые SQL-запросы к
> SQLite допустимы только для отладки и диагностики.

## Подключение к Copilot

Идея: агент (например, GitHub Copilot CLI) в начале сессии читает контекст
проекта через `mem-cli`, а по ходу работы дописывает новые факты и решения.

1. Соберите и положите бинарь в `PATH` (см. «Установка»).

2. Зафиксируйте каталог БД для проекта, чтобы он был стабильным между
   сессиями (например, в `.envrc`/окружении оболочки):

   ```sh
   export MEMORY_DB_DIR="$PWD/.memory"
   ```

3. Добавьте инструкции для агента в `AGENTS.md` (или в custom instructions
   Copilot) в корне проекта:

   ```markdown
   ## Контекст проекта (mem-cli)
   - В начале работы прочитать контекст:
     `mem-cli list facts`, `mem-cli decisions`, `mem-cli modules`,
     `mem-cli list commands`.
   - Новые стабильные факты сохранять: `mem-cli add facts "..."`.
   - Принятые технические решения: `mem-cli add decisions "..."`.
   - Проверенные команды build/test/lint: `mem-cli add commands "..."`.
   - Каталог БД задаётся переменной `MEMORY_DB_DIR` (по умолчанию `.memory/`).
   ```

4. Для машиночитаемого вывода используйте флаг `--json`:

   ```sh
   mem-cli list facts --json
   ```

## Разработка

```sh
cargo build
cargo test
cargo clippy -- -D warnings
cargo fmt
```
