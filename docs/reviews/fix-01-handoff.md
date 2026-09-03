# Handoff: fix-01-core-codecs

Branch: `task/fix-01-core-codecs`, base: `master@7d143fc`.
Scope: ревью-дефекты TASK-01 (ядро `boa_idb_core`: `key/*`, `clone/*`).

## Что исправлено (все пункты — регрессионные тесты в комплекте)

1. KEY-декодер принимал нетерминированные строки/бинарь (`key/encode.rs`) —
   добавлен флаг `found_terminator`, без терминатора `InvalidEncoding`.
2. KEY-декодер без лимита глубины — `MAX_KEY_DECODE_DEPTH = 32`
   (зеркалит дефолт `LimitConfig::max_key_depth`), плюс reject `NaN` для
   Number/Date (`InvalidValue`).
3. SCF memo на encode отсутствовал — добавлен `ScValue::MemoRef(usize)`
   (по уточнению №2 REVIEW-01): L1-конвертер сможет маркировать
   разделяемые/циклические графы; энкодер пишет `TAG_MEMO_REF`, декодер
   резолвит (был) с `InvalidMemoRef` на прямые/внедиапазонные ссылки.
4. Рассинхрон memo-индексов — обе стороны резервируют слоты для всех 9
   категорий ТЗ §7.5: Array, Object, Map, Set, Error, ArrayBuffer,
   TypedArray, DataView, Boxed (раньше decode не резервировал
   TypedArray/DataView/Boxed, encode — ArrayBuffer/TypedArray/DataView/Boxed).
5. Неверный CRC — `crc32fast::hash` (IEEE) заменён софтверным CRC32C
   (Castagnoli, новый модуль `clone/crc32c.rs`, вектор `0xE3069283`).
   Без новых зависимостей — ADR не требуется.
6. Аллокации до проверки лимита в SCF-decode — `u64→usize` через
   `try_from` (корректно и на 32-bit/wasm32), `checked_mul` для `len*2`,
   все dense-счётчики сверяются с remaining-входом до `with_capacity`,
   длина Array ограничена `max_value_len`; битые индексы/дубли/
   `items>length` → `CorruptedPayload`; знак BigInt `>1` и флаги RegExp
   `>0xFF` → `CorruptedPayload`.
7. `unreachable!()` в `compare_keys` заменён детерминированным
   `Ordering::Equal`-fallback с комментарием (кросс-типовые пары исключены
   проверкой `type_order` выше).
8. KeyPath `length` обрывал остаток пути — теперь `length` продолжает
   траверс (`items.length.foo` → `None`, `items.length` → число).
9. KeyPath через lossy-строку — обход и inject идут по raw code units
   (`split on U+002E`); добавлен публичный
   `KeyPath::validate_identifier_name(&[u16])` с декодированием суррогатных
   пар (lone-суррогаты → `InvalidSyntax`).
10. `unicode_id_start`: убраны `U+200E/U+200F` (bidi-контроли — не `ID_Start`
    по UAX #31). Полные таблицы (`unicode-ident`) сознательно не тянул:
    новая зависимость ради валидации; текущие таблицы — документированная
    аппроксимация, см. «Известные ограничения».
11. `Utf16String::from_str` добавлен (сигнатура по ТЗ §4.3),
    `from_rust_str` — алиас; `#[allow(clippy::should_implement_trait)]`
    с обоснованием. `decode_scf`: минимум `12→13`, порядок
    magic→version→длина (тест `scf_invalid_magic` снова зелёный).
12. Формат Boxed задокументирован (`TAG_BOXED` + subtag + полное tagged
    значение) в `encode.rs`.
13. Попутно: `deny.toml` — удалён deprecated-ключ `unlicensed` (конфиг не
    валидировался текущим cargo-deny); `integration_tests.rs` — порядок
    импортов (workspace `fmt --check`).

## Верификация (всё EXIT 0, кроме оговорённого deny)

- `cargo fmt --all -- --check` — чисто
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — 0
- `cargo test --workspace` — всё ok (core: 34+2+39+31+2+41, остальное без регрессий)
- `cargo check -p boa_idb_core --target wasm32-unknown-unknown` — ok
- `cargo doc -p boa_idb_core --no-deps` — ok
- grep `unwrap/expect/panic/unreachable/todo/unsafe` в `key/*`, `clone/*` — чисто
- `cargo deny check licenses` — конфиг починен, но остались **rejected:
  Unicode-3.0** (ICU через `boa_engine`) — см. ниже

## Известные ограничения / передано дальше

- `cargo deny`: `Unicode-3.0` от транзитивных ICU-крейтов `boa_engine`.
  Расширение allow-листа — лицензионное решение (нужен ADR по AGENTS.md п.5),
  относится к work order, притянувшему `boa_engine` (fix-03), — не стал
  втихаря дописывать в fix-01.
- `engine/scheduler.rs:51` — `unwrap()` в lib-коде (`poll_ready`); чинится в
  fix-02 (D15).
- Sparse-массивы: слоты `length` ограничены сверху `max_value_len`
  (`ValueTooLarge`); усилитель «дыры почти ничего не стоят во входе»
  задокументирован в коде — полный фикс (sparse-представление) при
  необходимости отдельным order'ом.
- Юникод-таблицы ID_Start/Continue — ручная аппроксимация UAX#31;
  точное покрытие (`unicode-ident` + тесты по DerivedCoreProperties) —
  опциональный follow-up.
- Proptest-генераторы (§9 ТЗ: `U+0000`, длинные строки, Map/Error/TypedArray
  в SCF-наборе) расширены лишь частично новыми unit-тестами; полное
  расширение генераторов — follow-up.

## Demo

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p boa_idb_core
cargo check --package boa_idb_core --target wasm32-unknown-unknown
```
