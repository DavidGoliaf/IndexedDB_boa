# ТЕХНИЧЕСКОЕ ЗАДАНИЕ: TASK-08 / M6-B

## Файловый backend — segments, compaction, MVCC и crash-safety

| Метаданные | Значение |
|---|---|
| **Нормативный этап** | M6 из `TZ_boa_idb_IndexedDB.md`, R8.3.3, R8.3.4, R8.3.6, R8.5.1–R8.5.3 и WPT-цель M6 |
| **Идентификатор work order** | `TASK-08-M6B-FILESYSTEM-COMPLETION` |
| **Ветка** | `task/m6-fs-completion`, от принятого `task/m6-fs-foundation` revision `94b4bff` |
| **Целевой crate** | `crates/boa_idb_fs`; при необходимости — изолированные изменения `boa_idb_wpt` |
| **Предусловие** | M6-A принят; его `WAL`, `LOCK`, schema/meta и лимит ключей являются совместимой базой |
| **Результат** | Нормативно завершённый файловый backend, а не новый формат API IndexedDB |

## 1. Контекст и граница с M6-A

M6-A уже предоставляет однописательный persistent backend, WAL с CRC и
recovery, advisory lock, strict/relaxed durability и временный индекс в памяти.
Он **не** закрывает immutable segments, compaction, быстрые readonly snapshots,
полную fault/crash матрицу и FS в WPT runner. Именно эти долги закрывает M6-B.

Не менять форматы SCF/KEY и публичный JS API. Не снижать и не переписывать
`expectations.json` без отдельного review. Не объявлять M7-бенчмарки
завершёнными: в этом наряде требуются лишь измеримые proof-tests свойств M6.

### Обязательный scope gate

До реализации исполнитель фиксирует в `QUESTIONS.md` оценку diff по четырём
пакетам ниже. Если суммарно получается более ~3000 строк либо нельзя закончить
без смешивания независимых инвариантов, **останавливается до кода** и предлагает
последовательные поднаряды `M6-B1` (storage format + MVCC), `M6-B2`
(compaction + reclamation), `M6-B3` (fault/crash + WPT). Нельзя обходить это
правило одной большой поставкой. Если оценка укладывается, один M6-B handoff
допустим. Каждый поднаряд сохраняет все инварианты ниже и получает свой handoff.

## 2. Архитектурные требования

### 2.1 Immutable segments и manifest — R8.3.3

1. Ввести versioned immutable файлы `seg/<sequence>.seg` и manifest, который
   однозначно перечисляет актуальные сегменты и WAL generation. Формат обязан
   иметь magic/version, ограниченные длины и checksum либо эквивалентную
   проверяемую защиту от повреждения. Парсинг повреждённых данных не паникует.
2. `CURRENT` должен указывать только на полностью подготовленный manifest.
   Публикация нового состояния строго следует: temporary file → write → file
   sync → atomic rename → directory sync там, где безопасный API платформы это
   предоставляет. Платформенное ограничение должно быть явно записано в ADR и
   тестах; нельзя молча выдать отсутствующий directory sync за выполненный.
3. При пороге WAL по умолчанию **64 MiB или 10 000 frames** compaction
   запускается в том же IO-потоке только между write-транзакциями. Пороги
   конфигурируемы для быстрых тестов, production defaults нормативны.
4. Compaction переносит состояние в новый segment, публикует новый manifest и
   меняет/очищает WAL только после успешной публикации. Сбой в любой точке
   оставляет при reopen либо старое, либо новое полностью согласованное
   состояние — никогда гибрид. Операция должна быть прерываемой и не должна
   удерживать write path бесконечно.
5. Старые WAL и segments удаляются только после того, как их не держит ни один
   readonly snapshot. Нужны явные generation/refcount ownership rules; никаких
   delete-on-best-effort или cleanup по таймеру как доказательства безопасности.

### 2.2 Readonly MVCC snapshots — R8.3.4

1. Старт `TxnMode::ReadOnly` должен быть O(1) или O(log n) по числу records,
   не полным `BTreeMap` clone. Разрешён persistent immutable map (`im`/аналог)
   либо эквивалентный собственный дизайн; выбор, сложность, memory trade-off и
   лицензия новой crate до использования документируются в `docs/DECISIONS.md`.
2. Snapshot видит ровно одно консистентное поколение index/segment/WAL,
   продолжает читать его во время последующих commits и compaction и освобождает
   reference при завершении/abort/drop.
3. Write transaction не должен наблюдать частично опубликованную compaction.
   Существующие savepoint, atomic commit, index/key-generator semantics и
   `max_keys_in_memory` остаются без регрессии.
4. Добавить instrumented test seam (счётчик clone/visit либо generation handles),
   который доказывает отсутствие O(n) обхода на open readonly transaction без
   flaky timing assertions. Результат и методику указать в handoff.

### 2.3 FileSystem и fault injection — R8.5.2, R8.5.3

1. Весь production IO файлового backend проходит через внутренний trait
   `FileSystem`: read/open/create/write/short-write handling, sync, rename,
   truncate, remove, directory sync и необходимые операции lock. Реализация
   реальной ФС — default; тестовая `FaultInjectingFs` не должна менять
   production control flow.
2. Fault layer должна детерминированно уметь инъецировать как минимум `ENOSPC`,
   `EIO`, короткую запись, ошибку sync, ошибку rename и задержку/прерывание в
   точках WAL, segment, manifest и cleanup. Каждая ошибка переводится в
   подходящий `BackendError`; production path без `unwrap`/`expect`/`panic!`.
3. Для каждого пункта публикации создать table-driven tests: после fault и
   reopen данные — только некоторый committed prefix, индексы согласованы,
   хвост WAL/temporary files безопасно обработаны. Повреждение последних N
   байт WAL **и** segment/manifest либо восстанавливается в valid prefix, либо
   возвращает нормализованную `NotReadableError`-цепочку; мусор не выдаётся.

### 2.4 Межпроцессный crash test и lock — R8.3.6, R8.5.1

1. Добавить отдельный worker binary/test helper, который выполняет повторяемые
   серии transaction commits через FS. Родитель завершает worker в одной из
   отмеченных точек, затем открывает базу и проверяет committed-prefix и
   согласованность primary/index/key-generator данных.
2. Nightly job выполняет минимум 200 seeded iterations. Обычный CI запускает
   компактный deterministic subset; seed, число итераций и команда nightly
   документируются. Падение выводит seed и replay command.
3. На Unix использовать эквивалент `SIGKILL`; на Windows — принудительное
   завершение процесса. В обоих случаях после смерти worker ОС освобождает
   `LOCK`, а новый open делает recovery. Если платформенный primitive не
   обеспечивает это в CI, зафиксировать конкретное ограничение и сделать test
   `cfg`-условным; не заявлять R8.3.6/R8.5.1 PASS без работающей CI-проверки
   целевой платформы.

### 2.5 FS в conformance/differential runner

1. Подключить backend selector `fs` в `boa_idb_wpt` без изменения семантики
   memory/SQLite. Каждый WPT run использует изолированный temp-root и чисто
   освобождает handles/LOCK.
2. Запустить полный поддерживаемый WPT corpus на FS. Цель M6: не менее 92 %;
   baseline memory и SQLite не должны регрессировать. Любой FAIL/TIMEOUT
   фиксируется только в существующем строго проверяемом expectations workflow
   с обоснованием и отдельным review, не массовым update snapshot.
3. Расширить backend differential tests на FS: CRUD/schema/index/cursors,
   abort/savepoint, durability/reopen и указанные recovery варианты должны
   совпадать с моделью в пределах явно документированных durability гарантий.

## 3. Обязательные тесты и доказательства

Помимо существующих M6-A тестов добавить адресные тесты для:

- threshold compaction по bytes и frame count, publication order и reopen;
- interruption/fault в каждой стадии segment → manifest → CURRENT → WAL cleanup;
- удержание старого сегмента активным readonly snapshot и его reclamation после
  drop; snapshot isolation на concurrent write/compaction;
- O(1)/O(log n) readonly-start instrumented proof;
- malformed segment/manifest, torn tail на всех поколениях и отсутствие panic;
- `ENOSPC`, `EIO`, short write, sync/rename/truncate/remove fault matrix;
- lock release после forced worker termination и committed-prefix recovery;
- 200-iteration seeded nightly crash suite;
- полный WPT run c `--backend fs` и expanded differential tests.

Property tests обязаны иметь bounded input/size и не создавать неограниченные
файлы. Тесты не должны опираться на `sleep` или вероятность scheduling.

## 4. Документация и трассируемость

До первого использования новой crate добавить ADR с поддерживаемостью,
популярностью и permissive license. Обновить `crates/boa_idb_fs/README.md`:
топология файлов, generation lifecycle, recovery guarantees, `max_keys` оценка,
durability уровни и известные платформенные особенности.

В `docs/traceability.md` перевести в `PASS` только те из R8.3.3, R8.3.4,
R8.3.6, R8.5.1–R8.5.3, которые имеют точные passing tests и команды. Если
nightly/platform gate ещё не подключён, оставить соответствующий requirement
`PARTIAL` с конкретной причиной. В конце написать
`docs/reviews/M6B-handoff.md`: commits, выбранный storage/MVCC design, команды,
фактические WPT цифры по всем backend, crash seeds/iterations, fault coverage,
платформенные отклонения и остатки (если они есть). Нельзя называть M6
принятым до независимого review.

## 5. Критерии приёмки

- [ ] Segment/manifest generations, безопасная publication и interruptible
  compaction работают без потери committed data.
- [ ] Readonly snapshot стартует O(1)/O(log n), изолирован от writes и защищает
  свои segments от premature deletion.
- [ ] Все IO идут через `FileSystem`; fault matrix покрывает WAL, segment,
  manifest и cleanup.
- [ ] Crash worker и lock-recovery работают на CI-платформах; nightly выполняет
  200 seeded iterations либо limitation оставлен честно `PARTIAL`.
- [ ] FS поддержан WPT runner; полный FS run ≥92 %, memory/SQLite без регрессии.
- [ ] Differential parity и все новые behavior changes имеют tests + trace.
- [ ] ADR, README, traceability и `M6B-handoff.md` обновлены без ложного PASS.
- [ ] `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
  `cargo test --workspace`, `cargo doc --workspace --no-deps` и
  `cargo deny check` проходят; в handoff приведены воспроизводимые команды.

## 6. Стоп-условия

Немедленно остановиться и задать вопрос, если требуется `unsafe`, FFI,
platform-specific undocumented syscall, несовместимое изменение формата
M6-A, изменение public API/SCF/KEY, непермиссивная dependency, правка WPT
expectations или прогноз diff выше лимита work order. Не подменять failure
подавлением ошибки, best-effort cleanup или удалением test case.

## 7. Рекомендуемый исполнитель

Требуется `gpt-5.6-sol` с reasoning **xhigh** (минимум high). Работа сочетает
Rust storage design, atomic filesystem publication, ownership/MVCC, Windows и
Unix process semantics, fault injection и WPT infrastructure. Более слабой
модели можно делегировать только заранее изолированные property/fault tests
после принятого ADR и интерфейсов; ей не следует самостоятельно выбирать
формат segments, правила reclamation или crash protocol.
