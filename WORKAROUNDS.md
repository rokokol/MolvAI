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
