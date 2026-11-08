// SPDX-License-Identifier: MIT
// Where the post-processing key comes from, and which field follows the choice
//
// The values are the ones that `molva_core::config::Config::validate_llm` accepts. `file` is
// kept only for a settings file that already uses it: the page offers the three sources the
// owner named and never rewrites a stored value behind the user's back

import type { ProviderKeyStatus } from "../types";

export type KeyField = "store" | "plain" | "env" | "none" | "legacy";

/** The sources that the page offers, in the order of the choice */
export const KEY_SOURCE_ORDER = ["keyring", "plain", "env", "none"] as const;

/**
 * The choices of the select: the fixed order, plus `legacy` when the file already says
 * something the page does not offer. Without that, the select would show an empty value
 */
export function keySourceChoices(current: string): string[] {
  const choices: string[] = [...KEY_SOURCE_ORDER];
  const stored = current.trim();
  if (stored !== "" && !choices.includes(stored)) {
    choices.push(stored);
  }
  return choices;
}

/** The field that the chosen source needs */
export function keyField(source: string): KeyField {
  switch (source.trim()) {
    case "keyring":
      return "store";
    case "plain":
      return "plain";
    case "env":
      return "env";
    case "none":
      return "none";
    default:
      return "legacy";
  }
}

/** Whether a pasted key goes to the OS key store through a command */
export function storesInKeyring(source: string): boolean {
  return keyField(source) === "store";
}

/** The sources whose key is written in clear text, which the page must say out loud */
export function warnsAboutClearText(source: string): boolean {
  return keyField(source) === "plain";
}

/** The key store refused: the page names the entry and offers the environment variable */
export function storeUnavailable(status: Pick<ProviderKeyStatus, "detail">): boolean {
  return status.detail !== undefined && status.detail !== "";
}
