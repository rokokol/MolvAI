// SPDX-License-Identifier: MIT
// The path input with suggestions while typing.
//
// The listing comes from Rust: one `read_dir` per pause in typing, no shell and no blocking of
// the input. Choosing an entry only fills the field — starting is still the button's job or
// Enter without a highlighted entry. A directory ends with a separator, so the next pause lists
// what is inside it.

import { useEffect, useId, useRef, useState } from "react";

import { useI18n } from "../i18n";
import type { PathSuggestion } from "../types";
import { comboboxAction, moveActive } from "./combobox";

/** How long typing pauses before the directory is read */
const DEBOUNCE_MS = 150;

export interface PathInputProps {
  id: string;
  value: string;
  onChange: (value: string) => void;
  /** Start the transcription of the current value, when the caller has a start action */
  onStart?: () => void;
  suggest: (input: string) => Promise<PathSuggestion[]>;
  disabled?: boolean;
}

export default function PathInput({
  id,
  value,
  onChange,
  onStart,
  suggest,
  disabled = false,
}: PathInputProps) {
  const { t } = useI18n();
  const listId = useId();
  const [options, setOptions] = useState<PathSuggestion[]>([]);
  const [active, setActive] = useState(-1);
  const [focused, setFocused] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const [loading, setLoading] = useState(false);
  const [failed, setFailed] = useState(false);
  const requests = useRef(0);

  useEffect(() => {
    if (value.trim() === "") {
      setOptions([]);
      setLoading(false);
      setFailed(false);
      return;
    }
    const current = requests.current + 1;
    requests.current = current;
    const timer = window.setTimeout(() => {
      setLoading(true);
      suggest(value)
        .then((list) => {
          if (current !== requests.current) {
            return;
          }
          setOptions(list);
          setFailed(false);
          setActive(-1);
        })
        .catch(() => {
          if (current !== requests.current) {
            return;
          }
          // An unreadable directory is not an error of the window: the field keeps working
          setOptions([]);
          setFailed(true);
        })
        .finally(() => {
          if (current === requests.current) {
            setLoading(false);
          }
        });
    }, DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [value, suggest]);

  const visible = focused && !dismissed && value.trim() !== "";

  const choose = (option: PathSuggestion) => {
    onChange(option.path);
    setActive(-1);
    // A directory opens for the next step; a file closes the list
    setDismissed(!option.is_dir);
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    const action = comboboxAction(event.key, {
      count: options.length,
      active,
      visible,
    });
    switch (action.type) {
      case "next":
        event.preventDefault();
        setDismissed(false);
        setActive((index) => moveActive(index, options.length, 1));
        return;
      case "previous":
        event.preventDefault();
        setDismissed(false);
        setActive((index) => moveActive(index, options.length, -1));
        return;
      case "close":
        event.preventDefault();
        setDismissed(true);
        setActive(-1);
        return;
      case "fill": {
        const target = options[action.index];
        if (target) {
          event.preventDefault();
          choose(target);
        }
        return;
      }
      case "start":
        onStart?.();
        return;
      default:
        return;
    }
  };

  return (
    <div className="combobox">
      <input
        id={id}
        type="text"
        role="combobox"
        aria-autocomplete="list"
        aria-expanded={visible}
        aria-controls={visible ? listId : undefined}
        aria-activedescendant={
          visible && active >= 0 ? `${listId}-option-${active}` : undefined
        }
        value={value}
        disabled={disabled}
        autoComplete="off"
        spellCheck={false}
        onChange={(event) => {
          setDismissed(false);
          setActive(-1);
          onChange(event.target.value);
        }}
        onKeyDown={onKeyDown}
        onFocus={() => setFocused(true)}
        onBlur={() => {
          setFocused(false);
          setActive(-1);
        }}
      />
      {visible && (
        <ul className="suggestions" id={listId} role="listbox" aria-label={t("files.suggestions")}>
          {options.map((option, index) => (
            <li
              key={option.path}
              id={`${listId}-option-${index}`}
              role="option"
              aria-selected={index === active}
              className={index === active ? "suggestion active" : "suggestion"}
              // Keeping the focus lets the click land before the list closes
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => choose(option)}
            >
              <span className="suggestion-name mono">{option.name}</span>
              <span className="small muted">
                {option.is_dir ? t("files.suggestionDir") : t("files.suggestionFile")}
              </span>
            </li>
          ))}
        </ul>
      )}
      <p className="small muted suggestions-status" role="status">
        {visible && options.length === 0
          ? loading
            ? t("common.loading")
            : failed
              ? t("files.suggestionsFailed")
              : t("files.suggestionsNone")
          : ""}
      </p>
    </div>
  );
}
