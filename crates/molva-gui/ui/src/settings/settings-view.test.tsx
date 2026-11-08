// SPDX-License-Identifier: MIT
// What the Settings page shows: one language field, visible checkboxes, a provider choice, a
// recorded hotkey instead of a text field, the key source and the MCP section

import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { I18nContext, translate } from "../i18n";
import type { Config } from "../types";
import Settings from "../views/Settings";

const i18n = {
  lang: "en" as const,
  t: (key: string, vars?: Record<string, string | number>) => translate("en", key, vars),
};

function config(change: (draft: Config) => void = () => undefined): Config {
  const draft: Config = {
    version: 1,
    ui_language: "auto",
    audio: {
      device: "default",
      gain: 1,
      max_duration_secs: 600,
      trim_silence: true,
      silence_threshold_db: -45,
      vad_min_pause_ms: 1500,
      noise_suppression: false,
      sounds: true,
      sound_volume: 0.4,
      warn_zero_level: true,
    },
    stt: {
      engine: "transcribe-cpp",
      model: "gigaam-v3-e2e-rnnt-q4_k_m",
      model_path: "",
      models_mirror: "",
      language: "ru",
      allowed_languages: ["ru", "en"],
      threads: 0,
      unload_after_secs: 600,
      no_speech_threshold: 0.6,
      auto_pull: true,
      chunked: true,
      chunk_pause_ms: 300,
      streaming_preview: true,
      streaming_inject: false,
      transcribe_cli: "",
      transcribe_backend: "auto",
      remote: {
        base_url: "https://api.openai.com/v1",
        api_key_source: "keyring",
        api_key_env: "OPENAI_API_KEY",
        model: "whisper-1",
      },
    },
    dictionary: { path: "", fuzzy: true, in_prompt: true },
    rules: {
      enabled: true,
      spoken_punctuation: true,
      auto_punctuation: true,
      remove_fillers: true,
      remove_repeats: true,
      numbers_as_digits: true,
      paragraph_pause_ms: 2000,
      llm_min_words: 10,
    },
    snippets: { enabled: true, items: [] },
    llm: {
      enabled: false,
      provider: "ollama",
      base_url: "http://localhost:11434/v1",
      model: "qwen3.5:4b",
      api_key_source: "keyring",
      api_key_env: "OPENAI_API_KEY",
      api_key_plain: "",
      data_collection: "deny",
      zdr: false,
      provider_order: "",
      allow_fallbacks: true,
      require_parameters: false,
      temperature: 0.2,
      max_tokens: 1024,
      timeout_secs: 20,
      max_retries: 2,
      reasoning_effort: "auto",
    },
    style: { default: "cleanup", by_app: {}, custom: [] },
    output: {
      mode: "auto",
      auto_type_max_chars: 200,
      restore_clipboard: true,
      restore_delay_ms: 400,
      paste_backend: "auto",
      type_backend: "auto",
      type_delay_ms: 4,
      pre_inject_delay_ms: 50,
      terminal_shortcut: true,
      notify_on_fallback: true,
    },
    hotkeys: {
      backend: "auto",
      push_to_talk: "F9",
      toggle: "Ctrl+Alt+Z",
      command: "Ctrl+Alt+X",
      cancel: "Ctrl+Alt+C",
      style_next: "Ctrl+Alt+S",
      tap_toggles: true,
      short_press_ms: 250,
      min_hold_ms: 200,
      double_tap_ms: 350,
    },
    command_mode: { enabled: true, system_prompt: "" },
    journal: {
      path: "",
      enabled: true,
      include_text: true,
      encrypt: false,
      key_source: "file",
      key_path: "",
      keep_audio: false,
      max_entries: 10000,
      max_size_mb: 50,
    },
    stats: { typing_baseline_wpm: 40 },
    privacy: {
      send_to_llm: true,
      cloud_consent: false,
      no_record_mode: false,
      telemetry: false,
    },
    mcp: { enabled: true },
    gui: { tray_animation: false, show_managed_settings: false },
    log: { level: "info", max_size_mb: 10 },
  };
  change(draft);
  return draft;
}

function render(view: Config): string {
  return renderToStaticMarkup(
    <I18nContext.Provider value={i18n}>
      <Settings
        config={view}
        status={null}
        reloadConfig={async () => undefined}
        refreshStatus={async () => undefined}
        onError={() => undefined}
        theme="system"
        onThemeChange={() => undefined}
      />
    </I18nContext.Provider>,
  );
}

describe("Settings: recognition", () => {
  it("shows the recognition language as removable chips", () => {
    const html = render(config());
    expect(html).toContain(translate("en", "settings.sttLanguage"));
    expect(html).toContain(translate("en", "settings.addLanguage"));
    expect(html).toContain("Russian");
    expect(html).not.toContain('id="stt-language"');
  });

  it("shows the detected languages when automatic detection is on", () => {
    const html = render(
      config((draft) => {
        draft.stt.language = "auto";
      }),
    );
    expect(html).toContain("Russian");
    expect(html).toContain("English");
    expect(html).toContain(translate("en", "settings.allowedLanguagesHint"));
  });

  it("offers the cloud API as an engine and shows the weights path", () => {
    const html = render(config());
    expect(html).toContain('id="stt-engine"');
    expect(html).toContain(translate("en", "settings.provider.remote"));
    expect(html).toContain('id="stt-model-path"');
  });
});

describe("Settings: the rules are checkboxes", () => {
  it("renders every rule as a checkbox with its label", () => {
    const html = render(config());
    for (const key of [
      "settings.rules.spokenPunctuation",
      "settings.rules.autoPunctuation",
      "settings.rules.removeFillers",
      "settings.rules.removeRepeats",
      "settings.rules.numbersAsDigits",
    ]) {
      expect(html, key).toContain(translate("en", key));
    }
    expect(html.match(/type="checkbox"/g)?.length ?? 0).toBeGreaterThanOrEqual(11);
  });
});

describe("Settings: hotkeys", () => {
  it("shows a recorder button instead of a text field", () => {
    const html = render(config());
    expect(html).toContain('id="hotkey-push_to_talk"');
    expect(html).not.toContain('type="text" value="F9"');
    expect(html).toContain("<kbd>F9</kbd>");
    expect(html).toContain(translate("en", "settings.hotkeys.record"));
  });
});

describe("Settings: the key source", () => {
  it("offers the key store, a plain key, the variable and no key", () => {
    const html = render(config());
    for (const key of [
      "settings.keySource.store",
      "settings.keySource.plain",
      "settings.keySource.env",
      "settings.keySource.none",
    ]) {
      expect(html, key).toContain(translate("en", key));
    }
  });

  it("says out loud that the plain source is clear text", () => {
    const html = render(
      config((draft) => {
        draft.llm.api_key_source = "plain";
      }),
    );
    expect(html).toContain(translate("en", "settings.keySource.plainWarning"));
  });
});

describe("Settings: MCP", () => {
  it("shows the toggle, the stdio command and the harness snippet", () => {
    const html = render(config());
    expect(html).toContain(translate("en", "settings.mcp.enabled"));
    expect(html).toContain("molva mcp");
    expect(html).toContain(translate("en", "settings.mcp.stdioHint"));
    expect(html).toContain(translate("en", "settings.mcp.harness.claudeCode"));
  });

  it("keeps the stored switch value", () => {
    const on = render(config());
    const off = render(
      config((draft) => {
        draft.mcp.enabled = false;
      }),
    );
    const before = (html: string) => {
      const label = translate("en", "settings.mcp.enabled");
      const at = html.indexOf(label);
      return html.slice(html.lastIndexOf("<input", at), at);
    };
    expect(before(on)).toContain('checked=""');
    expect(before(off)).not.toContain('checked=""');
  });
});

describe("Settings: cloud privacy", () => {
  it("shows the OpenRouter data collection choice for OpenRouter alone", () => {
    const local = render(config());
    expect(local).not.toContain(translate("en", "settings.llm.dataCollection"));

    const cloud = render(
      config((draft) => {
        draft.llm.provider = "openrouter";
        draft.llm.base_url = "https://openrouter.ai/api/v1";
      }),
    );
    expect(cloud).toContain(translate("en", "settings.llm.dataCollection"));
    expect(cloud).toContain(translate("en", "settings.llm.dataCollectionDeny"));
    expect(cloud).toContain(translate("en", "settings.llm.dataCollectionAllow"));
  });
});

describe("Settings: OpenRouter hosters", () => {
  const openrouter = () =>
    render(
      config((draft) => {
        draft.llm.provider = "openrouter";
        draft.llm.base_url = "https://openrouter.ai/api/v1";
      }),
    );

  it("shows the hoster section for OpenRouter alone", () => {
    const local = render(config());
    expect(local).not.toContain(translate("en", "settings.llm.hosters"));
    expect(local).not.toContain('id="llm-provider-order"');

    const html = openrouter();
    expect(html).toContain(translate("en", "settings.llm.hosters"));
    expect(html).toContain('id="llm-provider-order"');
    expect(html).toContain(translate("en", "settings.llm.hostersOrder"));
    expect(html).toContain(translate("en", "settings.llm.hostersOrderHint"));
  });

  it("shows both routing switches", () => {
    const html = openrouter();
    for (const key of [
      "settings.llm.allowFallbacks",
      "settings.llm.allowFallbacksHint",
      "settings.llm.requireParameters",
      "settings.llm.requireParametersHint",
    ]) {
      expect(html, key).toContain(translate("en", key));
    }
  });

  it("keeps the stored order in the field", () => {
    const html = render(
      config((draft) => {
        draft.llm.provider = "openrouter";
        draft.llm.provider_order = "groq, together";
      }),
    );
    expect(html).toContain('value="groq, together"');
  });

  it("drops the section when the provider is not OpenRouter", () => {
    const html = render(
      config((draft) => {
        draft.llm.provider = "groq";
        draft.llm.provider_order = "groq, together";
      }),
    );
    expect(html).not.toContain('id="llm-provider-order"');
  });
});

describe("Settings: the test buttons", () => {
  it("offers a model test and a recognition test in the page", () => {
    const html = render(config());
    expect(html).toContain(translate("en", "settings.llm.test"));
    expect(html).toContain(translate("en", "settings.llm.testHint"));
    expect(html).toContain(translate("en", "settings.sttTest.button"));
    expect(html).toContain(translate("en", "settings.sttTest.hint"));
  });

  it("does not save before a test: the buttons are plain buttons", () => {
    const html = render(config());
    const buttons = [...html.matchAll(/<button[^>]*>([^<]*)<\/button>/g)].map(
      (match) => match[1],
    );
    expect(buttons).toContain(translate("en", "settings.llm.test"));
    expect(buttons).toContain(translate("en", "settings.sttTest.button"));
    // A test button never submits a form
    expect(html).not.toContain('type="submit"');
  });
});

describe("Settings: custom styles", () => {
  it("shows the built-in defaults as not editable and the custom list separately", () => {
    const html = render(config());
    expect(html).toContain(translate("en", "settings.styles.builtin"));
    expect(html).toContain(translate("en", "settings.styles.builtinHint"));
    expect(html).not.toContain('id="style-id"');
  });

  it("renders an entry with its fields", () => {
    const html = render(
      config((draft) => {
        draft.style.custom = [
          { id: "polite", name: "Polite", uses_llm: true, system_prompt: "Be polite" },
        ];
      }),
    );
    expect(html).toContain('id="style-id"');
    expect(html).toContain('id="style-name"');
    expect(html).toContain('id="style-prompt"');
    expect(html).not.toContain(translate("en", "settings.styles.usesLlm"));
  });

  it("blocks the save while an entry has no prompt", () => {
    const html = render(
      config((draft) => {
        draft.style.custom = [
          { id: "polite", name: "Polite", uses_llm: true, system_prompt: "  " },
        ];
      }),
    );
    expect(html).toContain(translate("en", "settings.styles.issue.emptyPrompt"));
    expect(html).toContain(translate("en", "settings.styles.blocked"));
  });
});
