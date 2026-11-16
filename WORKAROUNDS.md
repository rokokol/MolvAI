# Workarounds

This file records arrangements that exist only because an external defect leaves no cleaner route. Each entry names the places it governs, the symptom it prevents, the mechanism, and the check that decides when the arrangement can go. A departure from an obvious route that is meant to stay belongs in `DEVIATIONS.md`, and a trap that stays reproducible belongs in `PITFALLS.md`; neither file exists yet, because nothing has needed one

---

## The GUI runs through XWayland on Hyprland

**Where:** `crates/molva-gui/src/main.rs`, in the Linux branch before GTK starts. It covers a Hyprland session when `GDK_BACKEND` is not already set by the user

**Symptom it prevents:** native Wayland with WebKitGTK 2.52.6 reports a negative `devicePixelRatio` (`-0.0104167`), `innerWidth = -86400`, `clientWidth = 900000002` and a computed `font-size` of 9,000,000 px. The panel then shrinks to an unreadable strip at the desktop breakpoint and native select popups are drawn against the wrong geometry. The same binary and the same CSS through XWayland report `941 / 1 / 64 px` and render normally

**Why it happens:** the WebKitGTK Wayland path reports inconsistent surface metrics to the WebView on this Hyprland and NVIDIA setup. The defect is outside MolvAI: the page receives impossible numbers before any CSS or React code runs. The current nixpkgs lock carries WebKitGTK 2.54.1; WebKitGTK 2.52.6 is the version where the measurements above were taken

**Why this works:** XWayland is a real X11 surface, and GTK's X11 backend gives WebKitGTK a consistent scale and viewport. The page still runs in the same WebView and no application behavior changes

**Rejected alternative:** a custom HTML dropdown drawn inside the page. It avoided the native GTK popup but the page geometry itself was still wrong under native Wayland, so the workaround only moved the symptom. Native `<select>` is the cleaner control once the backend is right

**Removal check:**

```sh
GDK_BACKEND=wayland WEBKIT_DISABLE_DMABUF_RENDERER=1 \
  ./target/debug/molva-gui
```

Open the window at 900×600 and inspect `window.devicePixelRatio`, `window.innerWidth` and the Settings dropdown. A positive device scale, a viewport close to 900 CSS px and a readable panel mean the backend can go back to Wayland. A negative scale, a viewport in the millions or a microscopic panel means the workaround stays

**Upstream:** not reported yet for this exact metric corruption. The closest open reports are Tauri [tauri-apps/tauri#5600](https://github.com/tauri-apps/tauri/issues/5600), "Incorrect DPI scaling on Wayland/Linux", and WebKit [bug 287811](https://bugs.webkit.org/show_bug.cgi?id=287811), "Incorrect devicePixelRatio on GTK/Wayland when using text-scaling-factor". Neither names WebKitGTK 2.52.6 on Hyprland with these exact values

---

## Hyprland changes the keyboard layout after a uinput paste

**Where:** `crates/molva-core/src/infra/inject/uinput.rs`, around `send_combo`. The paste and the copy of voice editing share that combination. It covers Hyprland; Sway, KDE and GNOME have best-effort guards in the same module

**Symptom it prevents:** a successful paste through `uinput` leaves the client, and often the layout indicator, on the US layout even when the physical keyboard was on Russian. The next typed letter is wrong until the user switches back

**Why it happens:** Hyprland gives the client the layout group of the keyboard that pressed the modifier. The virtual device `molvai-virtual-keyboard` starts on group 0, and its `Ctrl` makes the client use the US group. A group switch on the real keyboard does not reach the client while the virtual device owns the modifier

**Why this works:** before `Ctrl+V` the virtual device is switched to group 0, where the V key code is V, and after the paste it is switched back to the group of the physical keyboard. The physical keyboard itself is not touched. Sway uses `swaymsg input … xkb_switch_layout`, KDE uses `org.kde.KeyboardLayouts`, and GNOME uses the Shell input-source manager; a session without the command simply skips the guard

**Removal check:** on Hyprland, record a phrase with the physical keyboard on Russian, then run `hyprctl devices -j` and compare `active_keymap` for the physical keyboard and `molvai-virtual-keyboard`. Both must remain Russian after the phrase. Run voice editing once with a selection and repeat the comparison, because the copy combination takes the same path. The same check on Sway, KDE and GNOME decides whether their guard can stay

**Upstream:** Hyprland `InputManager` assigns the group of the keyboard that sent the modifier. The exact upstream report is not filed yet; the virtual device is a normal evdev keyboard from Hyprland's point of view, so the compositor has no way to know it is a paste helper

---

## The Nix package links the system AWS-LC

**Where:** `flake.nix`, the `buildInputs` of the `molva` package. It covers every build through `rustPlatform.buildRustPackage` whose nixpkgs carries [NixOS/nixpkgs#537108](https://github.com/NixOS/nixpkgs/pull/537108)

**Symptom it prevents:** the build stops in `aws-lc-sys-0.45.0` with `panicked at builder/main.rs:767: AWS_LC_SYS_USE_SYSTEM=1 requires a system-installed AWS-LC, but none was found`. The rejected candidate is `openssl`, because a plain OpenSSL has no `include/openssl/base.h`

**Why it happens:** nixpkgs' `rustPlatform.cargoSetupHook` now exports `AWS_LC_SYS_USE_SYSTEM=1`, `OPENSSL_NO_VENDOR=1` and the like into every cargo build. The tracking issues are [NixOS/nixpkgs#570938](https://github.com/NixOS/nixpkgs/issues/570938) and [NixOS/nixpkgs#570943](https://github.com/NixOS/nixpkgs/issues/570943). `aws-lc-sys` from 0.42 treats that value as a hard requirement, while the older versions fell back to compiling AWS-LC from source. An older nixpkgs, one without the hook, builds without the line. The line matters to a consumer whose nixpkgs carries it, and `huix` is one

**Why this works:** `aws-lc` in `buildInputs` puts `aws-lc.pc` on `PKG_CONFIG_PATH`. The build script takes that candidate, whose headers sit in `include/aws-lc/openssl`, and links `libcrypto-awslc` with `libssl-awslc` instead of compiling AWS-LC itself. nixpkgs fixes its own packages the same way, `rqbit` in [NixOS/nixpkgs#588523](https://github.com/NixOS/nixpkgs/pull/588523) among them

**Removal check:** build against a nixpkgs that carries the hook, with the `aws-lc` line still in place and then without it

```sh
nix build --no-link .#molva --override-input nixpkgs github:NixOS/nixpkgs/nixos-unstable
```

A build that passes without the line means the hook no longer demands a system AWS-LC, and the line can go. A panic naming `AWS_LC_SYS_USE_SYSTEM` means it must stay

**Upstream:** neither crate is at fault: `aws-lc-sys` documents `AWS_LC_SYS_USE_SYSTEM=1` as a hard requirement, and nixpkgs sets it deliberately. Moving this package into nixpkgs would inherit the fix from `default-crate-overrides.nix`, which passes `aws-lc` to `aws-lc-sys` from 0.42 on
