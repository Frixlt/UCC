# Справочная документация EC, ACPI, WMI и HID для платформ Uniwill (`UCC-Revorked`)

Clean-room спецификация аппаратных регистров, битовых полей, протоколов и аудита расхождений с кодовой базой `UCC-Revorked`, собранная по результатам кросс-сверки:
- живых замеров через `/dev/uwcc` на **MECHREVO YAOSHI Series-X6AR55xY** (`0x1A` / `IDY_7Y`, Intel Core Ultra 9 275HX + NVIDIA GeForce RTX 5080 Laptop GPU);
- официального драйвера ядра Linux [`drivers/platform/x86/uniwill/uniwill-acpi.c`](https://raw.githubusercontent.com/torvalds/linux/master/drivers/platform/x86/uniwill/uniwill-acpi.c) и [`uniwill-wmi.h`](https://raw.githubusercontent.com/torvalds/linux/master/drivers/platform/x86/uniwill/uniwill-wmi.h);
- документации и профилей [`open-uniwill-cc`](https://github.com/aspidovss/open-uniwill-cc/blob/main/docs/EC/Registers.ru.md) ([`Platforms.ru.md`](https://github.com/aspidovss/open-uniwill-cc/blob/main/docs/EC/Platforms.ru.md));
- профилей и протоколов [`MechControl-RE`](https://github.com/Liddo-kun/MechControl-RE/blob/main/Profiles/MECHREVO/IDY_7Y.json) ([`docs/architecture.md`](https://github.com/Liddo-kun/MechControl-RE/blob/main/docs/architecture.md)).

## Оглавление

| Документ | Область покрытия |
| --- | --- |
| [`UNIWILL_0X1A_REGISTERS.md`](./UNIWILL_0X1A_REGISTERS.md) | Оперативная сводка по `X6AR55xY` (`0x1A`), живые замеры (`123+ Вт`, `5133/5246 RPM`, `TCC 80 °C`), заводская таблица Turbo и переназначение кнопки `0x00B0`. |
| [`EC_01_POWER_AND_MODES.md`](./EC_01_POWER_AND_MODES.md) | Активация EC (`0x0706`/`0x0708`/`0x0709`/`0x0741` ACK), режимы `PMSC` (`0x0726`, `0x0727 CUME/PL4D`, `0x0751`), лимиты CPU (`PL1/PL2/PL4`, `PL4` x2, `TCC 0x0786/0x0463`, `Tau`, `0x0753/0x0754` VRM), лимиты GPU (`0x0743..0x0746`, `0x0788`/`0x07D5`, `SMRW`, `PWBT/PWDB/PWUP`). |
| [`EC_02_FANS_THERMAL_AND_COOLER.md`](./EC_02_FANS_THERMAL_AND_COOLER.md) | `RamFan 1.5` (`0x0F00..0x0F5F`, рукопожатие `0x0F5D..0x0F5F`), `RamFan 2` (`0x0F00..0x0FEF`), `CCI`, `0x07C5`/`0x07C6` (включая quirk `X6AR` bit 6 и Whisper Mode), `Fan Boost` (`0x0751 BIT(6)`), датчики/тахометры, СЖО (`0x07C7`/`0x07E6`/`0x07E7`). |
| [`EC_03_BATTERY_SWITCHES_MUX_WMI.md`](./EC_03_BATTERY_SWITCHES_MUX_WMI.md) | Пороги заряда (`0x07B9`/`0x07D0`), режимы защиты (`0x07A6`), Charge Hold, Type-C (`0x07CC`), телеметрия батареи/БП, переключатели (`Fn Lock 0x074E` vs `0x07A4`, `Win Lock`, USB Sleep, AC Auto Boot, Touchpad, Local Dimming), Display MUX (`MXDS`/`MXDM`) и полная таблица WMI-кодов. |
| [`EC_04_LIGHTING_AND_HID.md`](./EC_04_LIGHTING_AND_HID.md) | Полоса (`0x0748..0x074B`, `0x0767`/`0x0768`), логотип крышки (`0x0769..0x076B`, `0x07AB`, `0x078C`, `0x07C5 BIT(5)`), клавиатура ITE HID (`048D:600B`/`CE00`/`7000`/`7001`), `0x073C`/`0x0766`, `0x0741 BIT(3)` Reactive, и аудит `crates/ucc-lighting/`. |
| [`EC_05_CHASSIS_MATRIX_AND_AUDIT.md`](./EC_05_CHASSIS_MATRIX_AND_AUDIT.md) | Идентификация платформ (`0x0740` direct vs `<< 1`, `0x074C`, `0x07CA`, `0x0456`, `0x07D2`/`0x07D3`, `0x0782`, `0x078E`, `0x049F`), разграничение Intel `X6AR55xY` vs AMD `X6FR5xxY`, полный аудит `kernel/uwcc-rs/src/validation.rs` и приоритизированный список фиксов `UCC-Revorked`. |

## Ключевые пункты дорожной карты по итогам аудита

1. **Пользовательский лимит температуры CPU (`TCC` `0x0786` / `0x0463`):** подключить `read_tcc_offset` / `set_tcc_offset` в `uwcc/mod.rs` и вынести слайдер максимальной температуры (`80..=95 °C`) в редактор профиля (вместо дефолтных 80 °C, которые резали CPU флагом `THERM`).
2. **Живые лимиты EC на Dashboard (`M16.05`):** отображать актуальные `PL1/PL2/PL4`, действующий `TCC` (`0x0463`), `cTGP`/`DB` (`0x0788`/`0x07D5`) и активный режим `PMSC` (`Office / Gaming / Turbo / Custom`).
3. **Переназначение физической кнопки режима (`0x00B0`):** конфигурируемое действие при `0x0741 BIT(0)` (цикл профилей UCC включая `Custom`, быстрый переключатель или клавиша `uinput`).
4. **P0/P1 исправления по итогам 5 аудитов:** переписать `ramfan.rs` на реальную 96-байтную раскладку `0x0F00..0x0F5F`, исправить `SMRW` `data[4]`/`data[7]` и `PL4` x2 (`0x0727 BIT(7)`), уточнить `0x0743..0x0746`, `PWUP` max level (`<= 5`), `Fn Lock` (`0x074E BIT(4)`), нижний порог заряда (`0x07D0`), и фильтрацию HID-интерфейса `0xFF89` (`048D:7001`/`7000`) в `ucc-lighting`.
