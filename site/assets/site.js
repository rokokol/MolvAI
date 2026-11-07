// SPDX-License-Identifier: MIT
// Progressive enhancement only: the theme choice, the mobile menu, copy buttons and the
// install tabs. Every page stays usable with this file blocked

(function () {
  "use strict";

  var root = document.documentElement;
  root.classList.remove("no-js");
  root.classList.add("js");

  var STORAGE_KEY = "molvai-theme";
  var toggle = document.querySelector("[data-theme-toggle]");

  function stored() {
    try {
      return window.localStorage.getItem(STORAGE_KEY);
    } catch (error) {
      return null;
    }
  }

  function store(value) {
    try {
      window.localStorage.setItem(STORAGE_KEY, value);
    } catch (error) {
      /* A blocked storage must not break the page */
    }
  }

  function systemTheme() {
    return window.matchMedia && window.matchMedia("(prefers-color-scheme: light)").matches
      ? "light"
      : "dark";
  }

  function currentTheme() {
    return root.getAttribute("data-theme") || stored() || systemTheme();
  }

  function applyTheme(theme) {
    if (theme === "light" || theme === "dark") {
      root.setAttribute("data-theme", theme);
    }
  }

  applyTheme(stored());

  if (toggle) {
    toggle.addEventListener("click", function () {
      var next = currentTheme() === "dark" ? "light" : "dark";
      applyTheme(next);
      store(next);
    });
  }

  if (window.matchMedia) {
    var media = window.matchMedia("(prefers-color-scheme: light)");
    var onChange = function () {
      if (!stored()) {
        root.removeAttribute("data-theme");
      }
    };
    if (media.addEventListener) {
      media.addEventListener("change", onChange);
    } else if (media.addListener) {
      media.addListener(onChange);
    }
  }

  var menuButton = document.querySelector("[data-menu-toggle]");
  var nav = document.getElementById("site-nav");

  function closeMenu() {
    if (!nav || !menuButton) {
      return;
    }
    nav.classList.remove("is-open");
    menuButton.setAttribute("aria-expanded", "false");
  }

  if (menuButton && nav) {
    menuButton.addEventListener("click", function () {
      var open = nav.classList.toggle("is-open");
      menuButton.setAttribute("aria-expanded", open ? "true" : "false");
    });

    nav.addEventListener("click", function (event) {
      if (event.target.closest("a")) {
        closeMenu();
      }
    });

    document.addEventListener("keydown", function (event) {
      if (event.key === "Escape" && nav.classList.contains("is-open")) {
        closeMenu();
        menuButton.focus();
      }
    });

    document.addEventListener("click", function (event) {
      if (!nav.classList.contains("is-open")) {
        return;
      }
      if (!nav.contains(event.target) && !menuButton.contains(event.target)) {
        closeMenu();
      }
    });

    window.addEventListener("resize", function () {
      if (window.innerWidth > 900) {
        closeMenu();
      }
    });
  }

  function legacyCopy(text) {
    var area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.top = "-1000px";
    document.body.appendChild(area);
    area.select();
    var ok = false;
    try {
      ok = document.execCommand("copy");
    } catch (error) {
      ok = false;
    }
    document.body.removeChild(area);
    return ok;
  }

  function copy(text) {
    if (navigator.clipboard && window.isSecureContext) {
      return navigator.clipboard.writeText(text).then(
        function () {
          return true;
        },
        function () {
          return legacyCopy(text);
        }
      );
    }
    return Promise.resolve(legacyCopy(text));
  }

  Array.prototype.forEach.call(document.querySelectorAll("[data-copy]"), function (button) {
    button.addEventListener("click", function () {
      var text = button.getAttribute("data-copy") || "";
      var label = button.textContent;
      copy(text).then(function (ok) {
        button.textContent = ok ? "Copied" : "Press Ctrl+C";
        button.setAttribute("data-state", ok ? "copied" : "failed");
        window.setTimeout(function () {
          button.textContent = label;
          button.removeAttribute("data-state");
        }, 1800);
      });
    });
  });

  Array.prototype.forEach.call(document.querySelectorAll("[data-tabs]"), function (group) {
    var tabs = Array.prototype.slice.call(group.querySelectorAll('[role="tab"]'));
    var panels = Array.prototype.slice.call(group.querySelectorAll('[role="tabpanel"]'));
    if (!tabs.length || !panels.length) {
      return;
    }

    function activate(tab, focus) {
      tabs.forEach(function (item) {
        var active = item === tab;
        item.setAttribute("aria-selected", active ? "true" : "false");
        item.setAttribute("tabindex", active ? "0" : "-1");
      });
      panels.forEach(function (panel) {
        panel.classList.toggle("is-active", panel.id === tab.getAttribute("aria-controls"));
      });
      if (focus) {
        tab.focus();
      }
    }

    tabs.forEach(function (tab, index) {
      tab.addEventListener("click", function () {
        activate(tab, false);
      });
      tab.addEventListener("keydown", function (event) {
        var next = null;
        if (event.key === "ArrowRight") {
          next = tabs[(index + 1) % tabs.length];
        } else if (event.key === "ArrowLeft") {
          next = tabs[(index - 1 + tabs.length) % tabs.length];
        } else if (event.key === "Home") {
          next = tabs[0];
        } else if (event.key === "End") {
          next = tabs[tabs.length - 1];
        }
        if (next) {
          event.preventDefault();
          activate(next, true);
        }
      });
    });

    var selected = tabs.filter(function (tab) {
      return tab.getAttribute("aria-selected") === "true";
    })[0];
    activate(selected || tabs[0], false);
  });
})();
