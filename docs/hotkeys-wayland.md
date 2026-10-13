# Hotkeys on Wayland

On Wayland an application has no global hotkeys: the compositor owns the keyboard focus completely, and nobody can intercept a key combination "past" it. This is a protection and not a gap. So the main way to control MolvAI is the compositor binds that call `molva record ...`. The command `molva setup <compositor>` prints the same lines

The fallback is to read `/dev/input/event*` through evdev (`hotkeys.backend = "evdev"`). It sees the keys before the compositor and works even where there are no binds, but it needs permission to read the input devices. On NixOS this means membership in the `input` group. After you change the groups, log in again

## Push-to-talk, hands-free and command mode

Push-to-talk needs two events from the compositor: the press and the release. Hyprland (`bind`/`bindr`) and Sway (`bindsym` / `bindsym --release`) give both. KDE Plasma and GNOME do not report the key release at all, so only the `molva record toggle` switch is available there

The daemon itself tells a key hold from a short press. A hold shorter than `hotkeys.min_hold_ms` (200 ms) does not create a phrase. A press shorter than `hotkeys.short_press_ms` (250 ms) latches the recording when `hotkeys.tap_toggles = true`, and this is hands-free. A second press outside the `hotkeys.double_tap_ms` window ends the recording. Inside the window the daemon counts it as a bounce and ignores it

## Hyprland

```
# ~/.config/hypr/hyprland.conf
# Push-to-talk: bind fires on the press, bindr fires on the release.
bind  = , F9, exec, molva record start
bindr = , F9, exec, molva record stop

# Hands-free: one press starts the recording, the next press stops it.
bind  = CTRL SHIFT, space, exec, molva record toggle

# Command mode: voice editing of the selected text.
bind  = CTRL SHIFT ALT, space, exec, molva record start --mode command
bindr = CTRL SHIFT ALT, space, exec, molva record stop

# Cancel the current recording.
bind  = CTRL SHIFT, Escape, exec, molva record cancel

# The daemon must be running.
exec-once = molva daemon
```

Choose a push-to-talk key that has no other use: `F9`, `Pause` or `Control_R`. You set the right Ctrl as `bind = , Control_R, exec, molva record start`, because Hyprland accepts the keysym names from xkb

## Sway

```
# ~/.config/sway/config
# --no-repeat stops the auto-repeat from starting the recording again.
bindsym --no-repeat F9 exec molva record start
bindsym --release F9 exec molva record stop

bindsym --no-repeat Ctrl+Shift+space exec molva record toggle

bindsym --no-repeat Ctrl+Shift+Alt+space exec molva record start --mode command
bindsym --release Ctrl+Shift+Alt+space exec molva record stop

bindsym --no-repeat Ctrl+Shift+Escape exec molva record cancel

exec molva daemon
```

## KDE Plasma

Plasma does not tell the application about the key release, so push-to-talk does not work there. Only the toggle mode works

System Settings → Shortcuts → Add Command:

- command `molva record toggle`, combination `Ctrl+Shift+space`
- command `molva record cancel`, combination `Ctrl+Shift+Escape`

To start the daemon automatically: System Settings → Autostart → `molva daemon`

## GNOME

GNOME also gives only the press, so here too only the toggle works

```sh
path=/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/molva/
gsettings set org.gnome.settings-daemon.plugins.media-keys custom-keybindings "['$path']"
gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:$path name 'MolvAI toggle'
gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:$path command 'molva record toggle'
gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:$path binding '<Ctrl><Shift>space'
```

## If a bind does not fire

`molva doctor` prints the session, the compositor, the paste utilities it found, the availability of `/dev/uinput` and `/dev/input`, and whether the daemon answers on the socket. Check first that the daemon is running (`molva status`) and that `molva` is in the `PATH` of the environment from which the compositor starts the binds. On NixOS this is most often the reason for a "silent bind"
