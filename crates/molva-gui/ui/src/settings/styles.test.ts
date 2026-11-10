// SPDX-License-Identifier: MIT
// Rules of the custom style editor, mirroring `Config::validate_styles` of the core

import { describe, expect, it } from "vitest";

import type { Config, CustomStyle, StyleOption } from "../types";
import {
  canAddStyle,
  MAX_CUSTOM_STYLES,
  newCustomStyle,
  overridesBuiltin,
  renameStyleReferences,
  styleIssues,
  issueFor,
} from "./styles";

const builtins: StyleOption[] = [
  { id: "cleanup", name: "Cleanup" },
  { id: "formal", name: "Formal" },
];

function style(change: Partial<CustomStyle> = {}): CustomStyle {
  return {
    id: "polite",
    name: "Polite",
    uses_llm: true,
    system_prompt: "Rewrite the text politely",
    llm_min_words: 3,
    ...change,
  };
}

describe("styleIssues", () => {
  it("accepts a complete entry", () => {
    expect(styleIssues([style()])).toEqual([]);
  });

  it("needs an id, a name and a prompt for a style that calls the model", () => {
    const issues = styleIssues([style({ id: "  ", name: "", system_prompt: " " })]);
    expect(issueFor(issues, 0, "id")?.key).toBe("settings.styles.issue.emptyId");
    expect(issueFor(issues, 0, "name")?.key).toBe("settings.styles.issue.emptyName");
    expect(issueFor(issues, 0, "system_prompt")?.key).toBe(
      "settings.styles.issue.emptyPrompt",
    );
  });

  it("does not need a prompt for a style that works without the model", () => {
    expect(styleIssues([style({ uses_llm: false, system_prompt: "" })])).toEqual([]);
  });

  it("rejects a repeated id, and a trimmed one counts as the same", () => {
    const issues = styleIssues([
      style({ id: "polite" }),
      style({ id: " polite ", name: "Other" }),
    ]);
    expect(issueFor(issues, 1, "id")?.key).toBe("settings.styles.issue.duplicateId");
    expect(issueFor(issues, 0, "id")).toBeUndefined();
  });

  it("treats a different case as a different id, as the core does", () => {
    expect(styleIssues([style({ id: "polite" }), style({ id: "Polite" })])).toEqual([]);
  });

  it("reports the entry by its index", () => {
    const issues = styleIssues([style(), style({ id: "other", name: " " })]);
    expect(issues).toHaveLength(1);
    expect(issues[0].index).toBe(1);
    expect(issues[0].field).toBe("name");
  });

  it("refuses a word minimum that is not a whole non-negative number", () => {
    for (const bad of [-1, 1.5, Number.NaN]) {
      const issues = styleIssues([style({ llm_min_words: bad })]);
      expect(issueFor(issues, 0, "llm_min_words")?.key).toBe(
        "settings.styles.issue.minWords",
      );
    }
    expect(styleIssues([style({ llm_min_words: 0 })]).length).toBe(0);
  });
});

describe("canAddStyle", () => {
  it("offers slots up to the core limit and no more", () => {
    const full = Array.from({ length: MAX_CUSTOM_STYLES }, (_item, index) =>
      style({ id: `style-${index}` }),
    );
    expect(canAddStyle(full)).toBe(false);
    expect(canAddStyle(full.slice(1))).toBe(true);
  });
});

describe("newCustomStyle", () => {
  it("gives an id that no other entry uses", () => {
    const added = newCustomStyle([style({ id: "custom-2" })]);
    expect(added.id).toBe("custom-3");
    const issues = styleIssues([style({ id: "custom-2" }), added]);
    expect(issues.every((issue) => issue.field !== "id")).toBe(true);
  });

  it("starts with the model on and an empty prompt, which the form asks to fill", () => {
    const added = newCustomStyle([]);
    expect(added.uses_llm).toBe(true);
    expect(added.system_prompt).toBe("");
    expect(added.llm_min_words).toBe(3);
  });
});

describe("renameStyleReferences", () => {
  it("moves the default and the window rules to the new id", () => {
    const current: Config["style"] = {
      default: "polite",
      by_app: { kitty: "polite", firefox: "cleanup" },
      custom: [style({ id: "polite" })],
    };
    const next = renameStyleReferences(current, [style({ id: "brief" })]);
    expect(next.default).toBe("brief");
    expect(next.by_app).toEqual({ kitty: "brief", firefox: "cleanup" });
    expect(next.custom[0].id).toBe("brief");
  });

  it("leaves references alone when an entry is added or removed", () => {
    const current: Config["style"] = {
      default: "polite",
      by_app: { kitty: "polite" },
      custom: [style({ id: "polite" })],
    };
    const added = renameStyleReferences(current, [
      style({ id: "polite" }),
      style({ id: "brief" }),
    ]);
    expect(added.default).toBe("polite");
    expect(added.by_app).toEqual({ kitty: "polite" });
  });
});

describe("overridesBuiltin", () => {
  it("says when the entry takes a built-in place", () => {
    expect(overridesBuiltin("formal", builtins)).toBe(true);
    expect(overridesBuiltin(" formal ", builtins)).toBe(true);
    expect(overridesBuiltin("polite", builtins)).toBe(false);
  });
});
