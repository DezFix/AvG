# Как описать твик

Твики лежат в папке [`tweaks/`](../tweaks) в файлах `*.toml` и встраиваются в программу при сборке.
Свои наборы можно не собирать: положите `.toml`-файл в папку своих твиков (путь показан в
**Настройки → Твики**) и перезапустите AvG. Твик из этой папки с тем же `id` заменяет встроенный.

## Общие поля

```toml
[[tweak]]
id = "xiaomi.msa"                 # уникальный id: латиница, цифры, . _ -
title = "Рекламная служба MIUI"   # короткое название
description = "Что делает и чем рискуем — простыми словами."
category = "bloat"                # bloat | ui | performance | battery | network | privacy
risk = "safe"                     # safe | medium | high
profiles = ["weak", "privacy"]    # weak | battery | privacy (в «Свой набор» попадает всё)
vendors = ["xiaomi"]              # необязательно: xiaomi, samsung или ro.product.manufacturer в нижнем регистре
min_sdk = 26                      # необязательно: минимальная версия Android (API level)
max_sdk = 34                      # необязательно
default_on = true                 # необязательно: отмечен ли галочкой (рискованные — никогда)
action = { ... }                  # что сделать — см. ниже
```

**Уровень риска:**

- `safe` — ничего заметного не ломается.
- `medium` — может пропасть функция, которой кто-то пользуется: опишите какая.
- `high` — заметно меняет поведение телефона. Такой твик никогда не отмечается автоматически.

## Типы действий

### Удалить пакеты для пользователя

```toml
action = { type = "remove_packages", packages = ["com.miui.msa.global", "com.miui.systemAdSolution"] }
```

Команда: `pm uninstall -k --user 0 <пакет>`. Откат: `cmd package install-existing <пакет>`.
Пакеты, которых нет на телефоне, пропускаются, поэтому можно перечислять варианты для разных прошивок.

### Отключить пакеты

```toml
action = { type = "disable_packages", packages = ["com.example.app"] }
```

Команда: `pm disable-user --user 0`. Откат: `pm enable`.

### Изменить настройки

```toml
action = { type = "settings", values = [
  { namespace = "global", key = "window_animation_scale", value = "0.5" },
] }
```

`namespace` принимает `global`, `secure` или `system`. Перед изменением AvG читает текущее значение.
При откате оно возвращается, а если раньше настройки не было, она удаляется. Если на телефоне уже
стоят нужные значения, твик не показывается.

### Произвольные команды

```toml
action = { type = "commands", apply = ["cmd package compile -m speed-profile -a"], revert = ["cmd package compile --reset -a"] }
```

Пустой `revert` означает, что откат не нужен (например, очистка кэша). Используйте этот тип, только
если остальные не подходят.

### Уменьшить разрешение экрана

```toml
action = { type = "screen_scale", factor = 0.75 }
```

Откат: `wm size reset` и `wm density reset`.

## Проверка

```bash
cargo test -p avg-core
```

Тест `builtin_tweaks_are_valid` проверяет все встроенные файлы: синтаксис, допустимые имена пакетов и
значения, уникальность `id`. В pull request укажите модель и прошивку, на которых проверяли твик.
