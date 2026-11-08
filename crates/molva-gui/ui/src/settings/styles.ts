// SPDX-License-Identifier: MIT
// Custom post-processing styles: the form rules
//
// The core validates the same list before a save and rejects a broken one. These checks run
// in the form so a mistake shows at the field, and the Save button stays off until it is gone.
// `MAX_CUSTOM_STYLES` repeats the hard limit of `Config::validate_styles`: a form that offers
// an eleventh slot would fail at the save instead of at the button

import type { CustomStyle, StyleOption } from "../types";

/** The hard limit in `molva_core::config::Config::validate_styles` */
export const MAX_CUSTOM_STYLES = 10;

export type StyleField = "id" | "name" | "system_prompt";

export interface StyleIssue {
  index: number;
  field: StyleField;
  /** The dictionary key of the message */
  key: string;
}

/** Every rule of the custom list, in the order the fields appear */
export function styleIssues(styles: CustomStyle[]): StyleIssue[] {
  const issues: StyleIssue[] = [];
  const seen = new Set<string>();
  styles.forEach((style, index) => {
    const id = style.id.trim();
    if (id === "") {
      issues.push({ index, field: "id", key: "settings.styles.issue.emptyId" });
    } else if (seen.has(id)) {
      issues.push({ index, field: "id", key: "settings.styles.issue.duplicateId" });
    } else {
      seen.add(id);
    }
    if (style.name.trim() === "") {
      issues.push({ index, field: "name", key: "settings.styles.issue.emptyName" });
    }
    if (style.uses_llm && style.system_prompt.trim() === "") {
      issues.push({
        index,
        field: "system_prompt",
        key: "settings.styles.issue.emptyPrompt",
      });
    }
  });
  return issues;
}

/** The message of the first issue that belongs to one field of one entry */
export function issueFor(
  issues: StyleIssue[],
  index: number,
  field: StyleField,
): StyleIssue | undefined {
  return issues.find((issue) => issue.index === index && issue.field === field);
}

export function canAddStyle(styles: CustomStyle[]): boolean {
  return styles.length < MAX_CUSTOM_STYLES;
}

/**
 * A new entry with an id that no other entry uses, so the first save cannot fail on a
 * duplicate. A built-in id is allowed: it means "this built-in style, changed"
 */
export function newCustomStyle(styles: CustomStyle[]): CustomStyle {
  let index = styles.length + 1;
  while (styles.some((style) => style.id === `custom-${index}`)) {
    index += 1;
  }
  return {
    id: `custom-${index}`,
    name: "",
    uses_llm: true,
    system_prompt: "",
  };
}

/** Whether the entry takes the place of a built-in style of the same id */
export function overridesBuiltin(id: string, builtins: StyleOption[]): boolean {
  return builtins.some((style) => style.id === id.trim());
}
