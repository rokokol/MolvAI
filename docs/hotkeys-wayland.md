# Горячие клавиши на Wayland

На Wayland глобальных горячих клавиш у приложения нет: клавиатурный фокус целиком принадлежит композитору, и перехватить сочетание «мимо» него нельзя — это защита, а не недоработка. Поэтому основной способ управления MolvAI — бинды композитора, которые вызывают `molva record ...`. Эти же строки печатает `molva setup <композитор>`.

Запасной путь — чтение `/dev/input/event*` через evdev (`hotkeys.backend = "evdev"`): он видит клавиши до композитора и работает даже там, где биндов нет, но требует прав на устройства ввода. На NixOS это членство в группе `input`; после изменения групп нужно перелогиниться.

## Push-to-talk, hands-free и режим команд

Push-to-talk требует от композитора два события: нажатие и отпускание. Их дают Hyprland (`bind`/`bindr`) и Sway (`bindsym` / `bindsym --release`). KDE Plasma и GNOME отпускание клавиши не отдают вовсе, поэтому там доступен только переключатель `molva record toggle`.

Демон сам различает удержание и короткое нажатие: удержание короче `hotkeys.min_hold_ms` (200 мс) реплику не создаёт, а нажатие короче `hotkeys.short_press_ms` (250 мс) при `hotkeys.tap_toggles = true` защёлкивает запись — это и есть hands-free. Повторное нажатие вне окна `hotkeys.double_tap_ms` завершает её, а внутри окна считается дребезгом и игнорируется.

## Hyprland

```
# ~/.config/hypr/hyprland.conf
# Push-to-talk: bind срабатывает на нажатие, bindr — на отпускание.
bind  = , F9, exec, molva record start
bindr = , F9, exec, molva record stop

# Hands-free: одно нажатие включает запись, следующее — выключает.
bind  = CTRL SHIFT, space, exec, molva record toggle

# Режим команд: голосовая правка выделенного текста.
bind  = CTRL SHIFT ALT, space, exec, molva record start --mode command
bindr = CTRL SHIFT ALT, space, exec, molva record stop

# Отмена текущей записи.
bind  = CTRL SHIFT, Escape, exec, molva record cancel

# Демон должен быть запущен.
exec-once = molva daemon
```

Клавишу push-to-talk удобно брать ту, которой нет других применений: `F9`, `Pause` или `Control_R`. Правый Ctrl задаётся как `bind = , Control_R, exec, molva record start` — Hyprland принимает имена keysym из xkb.

## Sway

```
# ~/.config/sway/config
# --no-repeat не даёт автоповтору начинать запись заново.
bindsym --no-repeat F9 exec molva record start
bindsym --release F9 exec molva record stop

bindsym --no-repeat Ctrl+Shift+space exec molva record toggle

bindsym --no-repeat Ctrl+Shift+Alt+space exec molva record start --mode command
bindsym --release Ctrl+Shift+Alt+space exec molva record stop

bindsym --no-repeat Ctrl+Shift+Escape exec molva record cancel

exec molva daemon
```

## KDE Plasma

Plasma не сообщает приложению об отпускании клавиши, поэтому push-to-talk там не работает — рабочий режим только переключатель.

Параметры системы → Комбинации клавиш → Добавить команду:

- команда `molva record toggle`, комбинация `Ctrl+Shift+space`;
- команда `molva record cancel`, комбинация `Ctrl+Shift+Escape`.

Автозапуск демона: Параметры системы → Автозапуск → `molva daemon`.

## GNOME

GNOME тоже отдаёт только нажатие, поэтому здесь тоже переключатель.

```sh
path=/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/molva/
gsettings set org.gnome.settings-daemon.plugins.media-keys custom-keybindings "['$path']"
gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:$path name 'MolvAI toggle'
gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:$path command 'molva record toggle'
gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:$path binding '<Ctrl><Shift>space'
```

## Если бинд не срабатывает

`molva doctor` печатает сессию, композитор, найденные утилиты вставки, доступность `/dev/uinput` и `/dev/input` и отвечает ли демон на сокете. Первое, что стоит проверить: демон запущен (`molva status`), а `molva` есть в `PATH` того окружения, из которого композитор запускает бинды — на NixOS это чаще всего и есть причина «бинд молчит».
