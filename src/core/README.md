# Core Module: Data Structures & Parsing

Этот модуль содержит ключевые структуры данных, живущие в L1 кэше.

## L2OrderBook (`orderbook.rs`)
[См. предыдущие версии]

## Serializer (`serializer.rs`)

Сериализатор ордеров в JSON формат для API Bybit.

*   **Zero-Allocation:** Мы НЕ используем `serde_json::to_string` или макрос `format!`, так как они аллоцируют `String` в куче.
*   **Implementation:** Используем `std::io::Write` поверх мутабельного слайса байт (`&mut [u8]`) из стека.
*   **Numbers:** Для преобразования float/int в строку используется крейт `ryu` (самый быстрый float-to-string конвертер) и `itoa`, которые пишут напрямую в буфер.
