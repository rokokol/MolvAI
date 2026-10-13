# Installation by an AI agent

This file is a prompt. Open Claude Code (or another agent with terminal access) in the repository directory and say: "read docs/install.md and install MolvAI by this guide". Then the agent finds out which system you have and asks only the questions it cannot answer itself

If you have no agent, the "Installation" section of the [README](../README.md) describes the same steps in order, and you do them by hand

---

## Prompt for the agent

You install MolvAI, a system-wide voice input, on the machine of the user. Work step by step and do not skip the checks. After each step, say in a few words what the result was. Do not install anything in the system without the explicit consent of the user: first show the command, wait for the answer, then run it

### Step 1. Find out the environment

Collect the data and show it to the user as a table:

- OS and distribution: `uname -srm` and `cat /etc/os-release` on Linux, `sw_vers` on macOS
- Session type: `echo "$XDG_SESSION_TYPE"` gives `wayland` or `x11`
- Compositor or desktop environment: `echo "$XDG_CURRENT_DESKTOP"`. On Wayland also run `hyprctl version` or `echo "$HYPRLAND_INSTANCE_SIGNATURE"` for Hyprland, `gnome-shell --version`, `plasmashell --version`
- Sound server: `pactl info | head -n 3` shows PipeWire or PulseAudio
- Is Nix present: `nix --version`

The session type decides how to paste the text, and the sound server decides how to record. So you need these two answers exactly, not "probably"

### Step 2. Choose the installation method

Offer the user one of three methods and explain the difference in one sentence each:

1. **Nix** if `nix --version` answered. Nothing goes into the system: `nix develop` gives the toolchain and all libraries, and `cargo build --release` builds the binary
2. **`./install.sh`** if there is no Nix. The script checks the dependencies and installs nothing itself. It prints the exact commands for your distribution. After you install the dependencies, run the script again
3. **A ready binary** from the releases page, if you do not need to build

### Step 3. Install the dependencies

Run `./install.sh --check` and show the output. The script prints what is missing and commands like `  $ sudo apt install …`. Run exactly those commands and add nothing of your own. Guides for the systems:

- Debian, Ubuntu: `sudo apt install build-essential cmake pkg-config libasound2-dev libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxkbcommon-dev`
- Fedora: `sudo dnf install gcc-c++ cmake pkgconf-pkg-config alsa-lib-devel webkit2gtk4.1-devel gtk3-devel libappindicator-gtk3-devel librsvg2-devel libxkbcommon-devel`
- Arch: `sudo pacman -S base-devel cmake pkgconf alsa-lib webkit2gtk-4.1 gtk3 libayatana-appindicator librsvg libxkbcommon`
- macOS: `brew install cmake pkg-config`, and the Xcode Command Line Tools
- Windows: `winget install Rustlang.Rustup Kitware.CMake`, and the Build Tools for Visual Studio with the C++ workload

On Wayland you also need `wtype` and `wl-clipboard`. On X11 you need `xdotool` and `xclip`. Install only what matches the session that you found in step 1

### Step 4. Build and install

```sh
cargo build --release --bin molva
./install.sh
```

`install.sh` copies the binary to `~/.local/bin`, writes a manifest of the installed files, and needs no administrator rights. Check that `~/.local/bin` is in `PATH`. If it is not, show the user which line to add and to which shell file (`~/.bashrc`, `~/.zshrc`, `~/.config/fish/config.fish`)

Make sure that the binary answers: `molva --version`

### Step 5. Download the model

```sh
molva models pull small
```

Ask the user which model they need and explain the choice. `base` is faster and worse. `small` is a reasonable compromise and the default. `large-v3-turbo` is clearly more accurate but needs more memory and time. The weights come from Hugging Face and are checked against SHA-256. They are not part of the repository, and their licenses are in [model-licenses.md](model-licenses.md)

### Step 6. Choose the microphone

```sh
molva devices
```

Show the list to the user, ask which device to use, and write the choice to `audio.device`. Then check that a signal really comes from it:

```sh
molva record --seconds 3
```

A zero level while the user speaks does not mean "quiet". It means a wrong device or a muted channel in the mixer. Go back to the device choice and do not go further

### Step 7. Set up the hotkeys

Open `~/.config/molva/config.toml` and show the user the `[hotkeys]` section. By default, holding `F9` records, `Ctrl+Alt+Z` toggles, and `Ctrl+Alt+X` starts command mode on the selection. The combinations are simple on purpose, so that they work on any stand without a setup

The next step depends on the session that you found in step 1:

- **Hyprland 0.55 and later** (check with `hyprctl version`): run `molva setup hyprland --write`. It writes the binds to `~/.config/hypr/molvai.lua` and prints one `dofile(...)` line. Offer to add that line to `~/.config/hypr/hyprland.lua`, then run `hyprctl reload`. Explain that the file comes from the `[hotkeys]` settings, so the user changes the keys there and not in the file
- **Hyprland before 0.55**: the config is `~/.config/hypr/hyprland.conf`. Offer `bind = , F9, exec, molva record start` and `bindr = , F9, exec, molva record stop` for push-to-talk, where `bindr` fires on the release, plus `bind` lines for `molva record toggle` and `molva record toggle --mode command`

- **GNOME, KDE**: show how to create a custom shortcut in the keyboard settings for the commands `molva record start` and `molva record stop`
- **X11 without a desktop environment**: offer `sxhkd` or the built-in keyboard capture (`hotkeys.backend = "evdev"`, which needs membership in the `input` group)
- **Windows, macOS**: the application itself registers the global combination, so the system needs no separate setup. On macOS warn that the user must grant the "Accessibility" and "Input Monitoring" rights in the security settings

### Step 8. Check the paste

```sh
molva test-inject
```

The command pastes a test string into the active field and prints which method worked. Ask the user to open a text editor, put the cursor in the field and run the check

If the paste did not work: on Wayland make sure that `wtype` is installed. If the compositor does not allow input emulation, set `output.mode = "clipboard"`. The text then goes to the clipboard, and the user pastes it by hand. This is a working mode, not a defeat

### Step 9. Autostart, if needed

Ask whether to start MolvAI at login. If yes, run `./install.sh --autostart`. It creates an autostart file, and `./install.sh --uninstall` removes it

### Step 10. Report

Show the user the result: where the binary is, where the config is, which model was downloaded and where to, which device was chosen, which keys were assigned, and how to remove all this (`./install.sh --uninstall`). Remind the user about `molva stats` and `molva journal`, and that the phrase journal is in the home directory and goes nowhere

> [!NOTE]
> If a command did not work in some step, do not go to the next step and do not invent a workaround silently. Show the output to the user and explain the options
