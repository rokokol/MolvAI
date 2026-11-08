// SPDX-License-Identifier: MIT
// The site's own behaviour: the language switch, the copy buttons and the install tabs.
// Progressive enhancement: with this file blocked every page keeps its English text, the
// tabs render as stacked panels and no copy button appears. The kit's own behaviour — the
// clouds, the theme button and the spectrogram — lives in kit.js

(function () {
  "use strict";

  var root = document.documentElement;
  root.classList.remove("no-js");
  root.classList.add("js");

  // ---------- the language ----------
  //
  // English is in the markup and Russian lives beside it: an element carries data-ru with
  // the Russian HTML, and data-ru-<attribute> with the Russian of an aria-label, an alt, a
  // title or a meta description. The first pass remembers the English, so a switch is a
  // swap in both directions. With this file blocked nothing swaps and English stands

  var LANG_KEY = "molvai-lang";
  var ATTRS = ["aria-label", "alt", "title", "content"];
  var texts = [];
  var attributes = [];

  function each(list, fn) {
    Array.prototype.forEach.call(list, fn);
  }

  function collect() {
    each(document.querySelectorAll("[data-ru]"), function (el) {
      texts.push({
        el: el,
        isTitle: el.tagName === "TITLE",
        html: el.tagName === "TITLE" ? el.textContent : el.innerHTML,
      });
    });
    each(
      document.querySelectorAll(
        ATTRS.map(function (name) {
          return "[data-ru-" + name + "]";
        }).join(",")
      ),
      function (el) {
        ATTRS.forEach(function (name) {
          if (!el.hasAttribute("data-ru-" + name)) {
            return;
          }
          attributes.push({ el: el, name: name, value: el.getAttribute(name) || "" });
        });
      }
    );
  }

  function apply(lang) {
    var ru = lang === "ru";
    root.lang = ru ? "ru" : "en";
    root.setAttribute("data-lang", lang);

    texts.forEach(function (item) {
      var value = ru ? item.el.getAttribute("data-ru") : item.html;
      if (item.isTitle) {
        item.el.textContent = value;
      } else {
        item.el.innerHTML = value;
      }
    });

    attributes.forEach(function (item) {
      var value = ru ? item.el.getAttribute("data-ru-" + item.name) : item.value;
      if (value !== null) {
        item.el.setAttribute(item.name, value);
      }
    });

    each(document.querySelectorAll("[data-lang-set]"), function (button) {
      button.setAttribute(
        "aria-pressed",
        button.getAttribute("data-lang-set") === lang ? "true" : "false"
      );
    });

    var event;
    try {
      event = new CustomEvent("molvai:lang", { detail: lang });
    } catch (error) {
      return;
    }
    document.dispatchEvent(event);
  }

  function stored(key) {
    try {
      return window.localStorage.getItem(key);
    } catch (error) {
      return null;
    }
  }

  function store(key, value) {
    try {
      window.localStorage.setItem(key, value);
    } catch (error) {
      /* A blocked storage must not break the page */
    }
  }

  function queryLang() {
    var value;
    try {
      value = new URLSearchParams(window.location.search).get("lang");
    } catch (error) {
      return null;
    }
    return value === "ru" || value === "en" ? value : null;
  }

  function storedLang() {
    var value = stored(LANG_KEY);
    return value === "ru" || value === "en" ? value : null;
  }

  // The first supported language in the system preference wins: a system set to
  // en-US with Russian second opens in English, not in Russian. English is the fallback
  // when the list holds neither of the two languages the site has
  function browserLang() {
    var list = navigator.languages && navigator.languages.length
      ? navigator.languages
      : [navigator.language || "en"];
    for (var i = 0; i < list.length; i++) {
      var tag = String(list[i]).toLowerCase();
      if (/^ru\b/.test(tag)) {
        return "ru";
      }
      if (/^en\b/.test(tag)) {
        return "en";
      }
    }
    return "en";
  }

  function syncUrl(lang) {
    try {
      var url = new URL(window.location.href);
      if (url.searchParams.get("lang") === lang) {
        return;
      }
      url.searchParams.set("lang", lang);
      window.history.replaceState(null, "", url);
    } catch (error) {
      /* file:// and a blocked history must not break the page */
    }
  }

  var lang = queryLang() || storedLang() || browserLang();
  collect();
  apply(lang);
  if (queryLang()) {
    store(LANG_KEY, lang);
  }

  each(document.querySelectorAll("[data-lang-set]"), function (button) {
    button.addEventListener("click", function () {
      var next = button.getAttribute("data-lang-set");
      if (next !== "en" && next !== "ru") {
        return;
      }
      apply(next);
      store(LANG_KEY, next);
      syncUrl(next);
    });
  });

  // ---------- the copy buttons ----------

  // The button label is in the markup, so it is already in the chosen language when the
  // click arrives. The two state labels are the only strings that live here

  var WORDS = {
    en: { copied: "Copied", failed: "Press Ctrl+C" },
    ru: { copied: "Скопировано", failed: "Нажмите Ctrl+C" },
  };

  function stateWords() {
    return WORDS[root.lang === "ru" ? "ru" : "en"];
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

  each(document.querySelectorAll("[data-copy]"), function (button) {
    button.addEventListener("click", function () {
      var text = button.getAttribute("data-copy") || "";
      var label = button.textContent;
      copy(text).then(function (ok) {
        var words = stateWords();
        button.textContent = ok ? words.copied : words.failed;
        button.setAttribute("data-state", ok ? "copied" : "failed");
        window.setTimeout(function () {
          button.textContent = label;
          button.removeAttribute("data-state");
        }, 1800);
      });
    });
  });

  // ---------- the install tabs ----------

  each(document.querySelectorAll("[data-tabs]"), function (group) {
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
