# ТЕХНИЧЕСКОЕ ЗАДАНИЕ НА ИСПОЛНЕНИЕ: TASK-06

## Укрепление границы Structured Clone и гигиена dependency policy

| Метаданные | Значение |
|---|---|
| **Идентификатор** | `TASK-06-STRUCTURED-CLONE-HARDENING-AND-DEPENDENCY-HYGIENE` |
| **Этап ТЗ** | Пост-M5 stabilisation; требования `R5.0`, `R5.3`, `R5.11`, `R6.4`, `R10.2`, `R11`, `R13.3`, `R14` нормативного `TZ_boa_idb_IndexedDB.md` |
| **Ветка** | Создать от принятой ревизии M5: `task/m6-structured-clone-hardening` |
| **Целевые области** | `crates/boa_idb/src/convert/value.rs`, `crates/boa_idb_wpt/src/environment.rs`, адресные тесты `boa_idb`/`boa_idb_wpt`, `deny.toml`, handoff и traceability |
| **Не является целью** | Расширение WPT-набора, обновление Boa/rusqlite, унификация версий зависимостей, смена формата SCF-v1, новая браузерная API-поверхность |

## 1. Контекст и причина работ

M5 прошёл полный проверенный WPT-набор на memory и SQLite: 482/482 PASS.
Это не отменяет отдельной инженерной обязанности: распознавание поддерживаемых
WPT platform objects при structured clone должно быть аутентичным, а не
зависеть от доступного пользовательскому скрипту признака или имени
конструктора.

Текущий путь расположен в `convert/value.rs`: `PlatformCloneBrand`,
`has_platform_brand`, `non_serializable_platform_object` и
`platform_clone_name`. `environment.rs` создаёт JS-шимы и передаёт им symbol
brand через временное глобальное свойство. Свойство затем удаляется, но это
нужно доказывать автоматическими отрицательными тестами. Нельзя считать
«private symbol» не подделываемым только по комментарию.

Одновременно `cargo deny check` проходит, но сообщает два
`license-not-encountered`: `ISC` и `CC0-1.0` разрешены в `deny.toml`, хотя в
зафиксированном графе зависимостей их нет. Политика лицензий должна разрешать
только реально обоснованные лицензии. Предупреждения о нескольких версиях
пакетов — допустимый наблюдаемый долг и не входят в задачу: их устранение
требует несвязанного обновления транзитивных зависимостей.

## 2. Результат, который должен быть поставлен

1. Обычный JS-объект не может выдать себя за `DOMPoint`, `Blob`, `File`,
   `ImageData`, `DOMMatrix`, `DOMMatrixReadOnly`, `DOMRect` или
   `DOMRectReadOnly` и получить их platform-объектное clone-поведение.
2. Обычный JS-объект не может выдать себя за `Event`/`MessageChannel` и
   получить `DataCloneError` только из-за поддельных полей, prototype или
   имени конструктора.
3. Реальные WPT-шимы сохраняют уже подтверждённое M5 поведение: допустимые
   объекты проходят round-trip с корректным прототипом, а `Event` и
   `MessageChannel` корректно отклоняются.
4. После установки окружения тест не имеет способа получить внутренний brand
   через `globalThis`, enumerable/symbol properties shim instance или публичный
   API; если движок Boa не предоставляет требуемой изоляции, остановиться и
   зафиксировать техническое ограничение в `QUESTIONS.md`.
5. `deny.toml` не содержит неиспользуемых разрешений `ISC` и `CC0-1.0`;
   действующие `MPL-2.0` и `Unicode-3.0` остаются, поскольку обоснованы
   ADR-008. Никаких обновлений версий или правок `Cargo.lock` ради этого
   пункта.

## 3. Обязательные действия

### 3.1. Сначала доказать исходное состояние

Добавить либо сначала запустить адресные regression tests, показывающие
следующие сценарии. Тесты должны работать в том же `Context` и с тем же
`install_test_environment`, что WPT runner, а не в синтетическом отдельном
контексте.

- `structuredClone({ constructor: DOMPoint })` и другие варианты подмены
  prototype/constructor не создают platform clone.
- Попытка прочитать `globalThis.__boa_platform_clone_brand` после установки
  окружения не возвращает symbol; `Object.getOwnPropertySymbols(globalThis)`
  и `Object.getOwnPropertySymbols(shim)` не дают достаточного ключа для
  успешной подделки.
- Plain object с `postMessage`, с `constructor.name = "Event"` или
  `constructor.name = "MessageChannel"` сериализуется как обычный объект, а
  не отвергается как platform object.
- Реальные созданные shim-объекты сохраняют существующие M5 expectations:
  поддерживаемый тип после clone имеет ожидаемый prototype; `Event` и
  `MessageChannel` дают `DataCloneError`.

Если один из этих тестов уже проходит, он всё равно остаётся как регрессия,
защищающая контракт. Если не проходит — исправление должно быть минимальным
и объяснено в commit body и handoff.

### 3.2. Укрепить механизм brand-а

Выбрать минимальный корректный вариант, совместимый с Boa 0.22:

- Предпочтительно хранить identity/brand на стороне Rust (`Context` data или
  реестр `JsObject` identity), а не доверять JS-свойству, строковому имени или
  пользовательскому prototype.
- Если symbol остаётся частью реализации shim-а, он должен быть недоступен
  пользовательскому коду после bootstrap, а Rust-проверка должна явно
  валидировать identity реального shim-конструктора. Нельзя принимать
  `constructor.name`, доступное собственное поле или только факт наличия
  произвольного symbol.
- Не менять публичный IDB API и формат `ScValue` без отдельного ADR и
  согласования. Внутренний marker `\0boa-platform-clone-type` разрешается
  менять только при сохранении обратной совместимости хранения либо при
  отдельной миграционной стратегии.
- Ошибки из отражения/проверки prototype не должны маскироваться panic-ом;
  `unwrap`/`expect` в production-path не допускаются.

### 3.3. Сохранить conformance

После правки выполнить минимум:

```powershell
cargo test -p boa_idb --tests
cargo test -p boa_idb_wpt --tests
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend memory --timeout 30 --quiet --check-expectations
cargo run -p boa_idb_wpt --bin boa-idb-wpt -- --backend sqlite --timeout 30 --quiet --check-expectations
```

Нельзя обновлять `expectations.json`, чтобы скрыть регрессию. Любой FAIL,
TIMEOUT, NOTRUN или mismatch строгого snapshot является отказом задачи.

### 3.4. Минимизировать license allowlist

Удалить из `deny.toml` ровно `ISC` и `CC0-1.0`, затем выполнить:

```powershell
$env:CARGO_DENY_DB_PATH = Join-Path $PWD "target\cargo-deny-advisories"
cargo deny fetch db
cargo deny check
```

Итог допускает предупреждения `[duplicate]` для текущих транзитивных версий,
но не должен содержать `license-not-encountered`. Не менять
`multiple-versions = "warn"` на `deny` и не добавлять bypass/skip-листы.

## 4. Ограничения и решения, требующие эскалации

Остановиться и спросить заказчика до изменения, если необходимы:

- новая dependency или изменение version constraint;
- изменение SCF-v1 / персистентного формата либо миграция данных;
- изменение нормативной семантики structured clone;
- больше ~3000 строк diff;
- ослабление WPT expectations, отключение теста или принятие unverified
  исключения.

Новая dependency требует ADR в `docs/DECISIONS.md` до её использования.

## 5. Критерии приёмки

- [ ] Добавлены негативные тесты подделки identity/brand и позитивные тесты
  настоящих shim objects.
- [ ] Тесты доказывают, что user JS не использует остаточный brand после
  bootstrap.
- [ ] Существующие full WPT strict checks на обоих backend по-прежнему дают
  482 matched, 0 unexpected и exit code 0.
- [ ] `cargo deny check` даёт `advisories ok`, `bans ok`, `licenses ok`,
  `sources ok` без `license-not-encountered`; предупреждения duplicate
  документированы как не входящие в scope.
- [ ] Проходят `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
  `cargo test --workspace`, `cargo doc --workspace --no-deps`.
- [ ] Обновлены `docs/traceability.md` и
  `docs/reviews/M6-handoff.md`; в handoff указаны команда, краткий итог
  каждого запуска, изменения и отсутствие отклонений.
- [ ] Чистый `git status`; работа лежит в логичных commits, не в наборе
  незакоммиченных файлов.

## 6. Требуемый уровень агентской модели

**Рекомендация: сильная модель, уровень `gpt-5.6-sol` с reasoning `high` или
выше.** Это не задача на механическую замену в TOML: исполнитель должен
уверенно работать с Rust ownership/Boa `JsObject` identity, границами между
Rust и исполняемым JS, structured clone и негативным security-тестированием.

Модель среднего уровня допустима только для подзадачи allowlist после того,
как сильная модель зафиксирует design и тест-план. Быструю/мини-модель не
назначать владельцем: наиболее вероятная ошибка — «исправить» WPT зелёным
snapshot-ом либо перенести подделываемый brand в другое JS-свойство.

Выполнение одним сильным агентом предпочтительно: точки изменения тесно
связаны и требуют единого решения об identity. Независимый второй агент
уместен лишь как read-only reviewer после готового diff.
